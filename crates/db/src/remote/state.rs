use super::{
    CloseStreamReq, RemoteConnection, RemoteErrorKind, StreamRequest, StreamResponse, execute_sql,
    expect_ok,
};
use crate::{Error, Result, TransactionBehavior};
use libsql_hrana::proto::{PipelineRespBody, Value};
use reqwest::Url;

pub(super) struct Stream {
    pipeline: Url,
    baton: Option<String>,
}

impl RemoteConnection {
    pub(crate) fn begin(&self, behavior: TransactionBehavior) -> Result<()> {
        self.ensure_usable()?;
        if self.active.borrow().is_some() {
            return Err(Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_ERROR),
                Some("remote database transaction already active".into()),
            ));
        }
        let sql = match behavior {
            TransactionBehavior::Deferred => "BEGIN DEFERRED",
            TransactionBehavior::Immediate => "BEGIN IMMEDIATE",
            TransactionBehavior::Exclusive => "BEGIN EXCLUSIVE",
            _ => return Err(RemoteErrorKind::Unsupported.error()),
        };
        let mut stream = self.open_stream()?;
        let result = self.control(&mut stream, sql);
        if let Err(error) = result {
            if self.poisoned.get() {
                return Err(RemoteErrorKind::OutcomeUnknown.error());
            }
            self.close(&mut stream)?;
            return Err(error);
        }
        *self.active.borrow_mut() = Some(stream);
        Ok(())
    }

    pub(crate) fn finish(&self, commit: bool) -> Result<()> {
        self.ensure_usable()?;
        let mut stream = self
            .active
            .borrow_mut()
            .take()
            .ok_or_else(|| RemoteErrorKind::Unsupported.error())?;
        let result = self.control(&mut stream, if commit { "COMMIT" } else { "ROLLBACK" });
        if self.poisoned.get() {
            return Err(RemoteErrorKind::OutcomeUnknown.error());
        }
        if result.is_err() && (!commit || self.control(&mut stream, "ROLLBACK").is_err()) {
            // Do not retry failed cleanup. A known SQL failure still leaves
            // this guard unsafe; closing the stream is attempted once only.
            if !self.poisoned.get() {
                let _ = self.close(&mut stream);
            }
            self.poisoned.set(true);
            return Err(RemoteErrorKind::OutcomeUnknown.error());
        }
        self.close(&mut stream)?;
        result
    }

    pub(super) fn request(
        &self,
        request: StreamRequest,
        read_only: bool,
    ) -> Result<StreamResponse> {
        self.request_with_rowid(request, false, read_only)
            .map(|(response, _)| response)
    }

    pub(super) fn request_with_rowid(
        &self,
        request: StreamRequest,
        read_rowid: bool,
        read_only: bool,
    ) -> Result<(StreamResponse, Option<i64>)> {
        self.ensure_usable()?;
        let may_mutate = !read_only;
        let mut requests = vec![request];
        if read_rowid {
            requests.push(execute_sql("SELECT last_insert_rowid()", true));
        }
        let (response, closing) = if self.active.borrow().is_some() {
            let response = {
                let mut active = self.active.borrow_mut();
                self.send(active.as_mut().unwrap(), requests, false)
                    .map_err(|_| RemoteErrorKind::OutcomeUnknown.error())?
            };
            (response, false)
        } else {
            let mut stream = self.open_stream()?;
            requests.push(StreamRequest::Close(CloseStreamReq {}));
            let response = self.send(&mut stream, requests, true).map_err(|error| {
                if may_mutate {
                    RemoteErrorKind::OutcomeUnknown.error()
                } else {
                    error
                }
            })?;
            (response, true)
        };
        let mut results = response.results.into_iter();
        let result = expect_ok(results.next().unwrap());
        let rowid = if read_rowid {
            let result = expect_ok(results.next().unwrap());
            let Ok(StreamResponse::Execute(result)) = result else {
                return self.invalid_response();
            };
            if result.result.cols.len() != 1 {
                return self.invalid_response();
            }
            let [row] = result.result.rows.as_slice() else {
                return self.invalid_response();
            };
            let [Value::Integer { value }] = row.values.as_slice() else {
                return self.invalid_response();
            };
            Some(*value)
        } else {
            None
        };
        if closing && expect_ok(results.next().unwrap()).is_err() {
            return self.invalid_response();
        }
        result.map(|response| (response, rowid))
    }

    fn open_stream(&self) -> Result<Stream> {
        // A v2 pipeline executes later statements after a known SQL error.
        // Verify setup before any caller SQL is sent on the same stream.
        let setup = self.pipeline(
            &self.transport.pipeline,
            None,
            vec![
                execute_sql("PRAGMA foreign_keys=ON", false),
                execute_sql("PRAGMA foreign_keys", true),
            ],
            false,
        )?;
        let mut stream = Stream {
            pipeline: self.transport.next_pipeline(setup.base_url.as_deref())?,
            baton: setup.baton,
        };
        let mut results = setup.results.into_iter();
        let enabled = expect_ok(results.next().unwrap()).is_ok();
        let checked = expect_ok(results.next().unwrap());
        let verified = matches!(checked, Ok(StreamResponse::Execute(ref result))
            if result.result.rows.len() == 1
                && result.result.rows[0].values.len() == 1
                && matches!(result.result.rows[0].values[0], Value::Integer { value: 1 }));
        if !enabled || !verified {
            let _ = self.close(&mut stream);
            return self.invalid_response();
        }
        Ok(stream)
    }

    fn control(&self, stream: &mut Stream, sql: &str) -> Result<()> {
        let response = self.send(stream, vec![execute_sql(sql, false)], false)?;
        let result = expect_ok(response.results.into_iter().next().unwrap())?;
        match result {
            StreamResponse::Execute(result)
                if result.result.cols.is_empty() && result.result.rows.is_empty() =>
            {
                Ok(())
            }
            _ => self.invalid_response(),
        }
    }

    fn close(&self, stream: &mut Stream) -> Result<()> {
        let response = self
            .send(stream, vec![StreamRequest::Close(CloseStreamReq {})], true)
            .map_err(|_| RemoteErrorKind::OutcomeUnknown.error())?;
        if expect_ok(response.results.into_iter().next().unwrap()).is_err() {
            self.poisoned.set(true);
            return Err(RemoteErrorKind::OutcomeUnknown.error());
        }
        Ok(())
    }

    fn send(
        &self,
        stream: &mut Stream,
        requests: Vec<StreamRequest>,
        closing: bool,
    ) -> Result<PipelineRespBody> {
        let response = self.pipeline(&stream.pipeline, stream.baton.take(), requests, closing)?;
        if response.base_url.is_some() {
            stream.pipeline = self.transport.next_pipeline(response.base_url.as_deref())?;
        }
        stream.baton.clone_from(&response.baton);
        Ok(response)
    }
}

impl Drop for RemoteConnection {
    fn drop(&mut self) {
        if !self.poisoned.get() && self.active.get_mut().is_some() {
            let _ = self.finish(false);
        }
    }
}
