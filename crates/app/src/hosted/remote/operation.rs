//! One explicit completion protocol shared by remote HTTP and administration.
use std::{error::Error, fmt};

use meibo_core::{Engine, types::EngineError};
use meibo_db::{RemoteErrorKind, remote_error_kind};

use super::{
    super::{
        HostedResult,
        remote_gate::{GateError, GateGuard},
    },
    store::RemoteStore,
};

pub(in crate::hosted) const UNKNOWN_MESSAGE: &str =
    "処理結果を確認できません。再送せず管理者へ連絡してください。";

struct Uncertain(Box<dyn Error>);
impl fmt::Display for Uncertain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "outcome_unknown: {UNKNOWN_MESSAGE}")
    }
}
impl fmt::Debug for Uncertain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("UncertainRemoteOperation")
    }
}
impl Error for Uncertain {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.0.as_ref())
    }
}

pub(in crate::hosted) fn is_uncertain(error: &(dyn Error + 'static)) -> bool {
    if error.is::<Uncertain>()
        || error.downcast_ref::<GateError>() == Some(&GateError::UncertainConnection)
        || error.downcast_ref::<meibo_db::Error>().is_some_and(|error| {
            matches!(remote_error_kind(error), Some(RemoteErrorKind::OutcomeUnknown | RemoteErrorKind::ConnectionPoisoned))
                || matches!(error, meibo_db::Error::SqliteFailure(code, _) if code.code == meibo_db::ErrorCode::SystemIoFailure)
        })
    {
        return true;
    }
    error.source().is_some_and(is_uncertain)
}

pub(in crate::hosted) fn report<T>(result: HostedResult<T>) -> HostedResult<T> {
    result.map_err(|error| {
        if !error.is::<Uncertain>() && is_uncertain(error.as_ref()) {
            Box::new(Uncertain(error)) as Box<dyn Error>
        } else {
            error
        }
    })
}

/// A failed constructor has no Engine left to inspect. Retain ownership for
/// storage failures rather than inferring health from the missing object.
pub(in crate::hosted) fn failed_construction(error: &(dyn Error + 'static)) -> bool {
    is_uncertain(error)
        || matches!(
            error.downcast_ref::<EngineError>(),
            Some(EngineError::Storage(_))
        )
        || error.source().is_some_and(failed_construction)
}

pub(in crate::hosted) struct RemoteOperation {
    guard: GateGuard,
    uncertain: bool,
}
impl RemoteOperation {
    pub(in crate::hosted) fn begin(store: &RemoteStore, label: &str) -> HostedResult<Self> {
        Ok(Self {
            guard: store.acquire_gate(label)?,
            uncertain: false,
        })
    }
    pub(in crate::hosted) fn construction<T>(
        &mut self,
        result: HostedResult<T>,
    ) -> HostedResult<T> {
        if result
            .as_ref()
            .is_err_and(|error| failed_construction(error.as_ref()))
        {
            self.uncertain = true;
        }
        result
    }
    pub(in crate::hosted) fn observe(&mut self, engine: &Engine) {
        self.uncertain |= engine.is_poisoned();
    }
    pub(in crate::hosted) fn observe_uncertainty(&mut self, uncertain: bool) {
        self.uncertain |= uncertain;
    }
    pub(in crate::hosted) fn finish<T>(
        self,
        store: &RemoteStore,
        result: HostedResult<T>,
    ) -> HostedResult<T> {
        if self.uncertain
            || store.conn.is_poisoned()
            || result
                .as_ref()
                .is_err_and(|error| is_uncertain(error.as_ref()))
        {
            // Guard drop performs no SQL, including after a lost reply.
            let error = result
                .err()
                .unwrap_or_else(|| GateError::UncertainConnection.into());
            return Err(Box::new(Uncertain(error)));
        }
        self.guard
            .release(store)
            .map_err(|error| Box::new(Uncertain(error)) as Box<dyn Error>)?;
        result
    }
}
