//! Local stdio adapter for MCP clients.
mod protocol;
mod tools;

use meibo_core::{Engine, OpenOptions};
use serde_json::Value;
use std::{
    error::Error,
    io::{BufRead, Write},
    path::Path,
};

const MAX_MESSAGE: usize = 1024 * 1024;

pub(crate) fn serve(db: &Path, allow_write: bool) -> Result<(), Box<dyn Error>> {
    let mut engine = Engine::open(
        db,
        &OpenOptions {
            cache_mib: 64,
            ..OpenOptions::default()
        },
    )
    .map_err(|_| "MCP用の既存DBを開けません。保存先を確認してください。")?;
    run_io(
        &mut engine,
        std::io::stdin().lock(),
        std::io::stdout().lock(),
        allow_write,
    )?;
    Ok(())
}

fn run_io(
    engine: &mut Engine,
    mut reader: impl BufRead,
    mut writer: impl Write,
    allow_write: bool,
) -> std::io::Result<()> {
    let mut protocol = protocol::Protocol::new(allow_write);
    while let Some(frame) = read_frame(&mut reader)? {
        let reply = match frame {
            Frame::Oversized => Some(protocol::error(
                Value::Null,
                -32600,
                "Message exceeds 1 MiB.",
                None,
            )),
            Frame::Message(bytes) => match serde_json::from_slice::<Value>(&bytes) {
                Ok(message) => protocol.handle(engine, &message),
                Err(_) => Some(protocol::error(Value::Null, -32700, "Parse error.", None)),
            },
        };
        if let Some(reply) = reply {
            serde_json::to_writer(&mut writer, &reply).map_err(std::io::Error::other)?;
            writer.write_all(b"\n")?;
            writer.flush()?;
        }
    }
    Ok(())
}

enum Frame {
    Message(Vec<u8>),
    Oversized,
}

fn read_frame(reader: &mut impl BufRead) -> std::io::Result<Option<Frame>> {
    let mut bytes = Vec::new();
    let mut oversized = false;
    loop {
        let chunk = reader.fill_buf()?;
        if chunk.is_empty() {
            return Ok(if oversized {
                Some(Frame::Oversized)
            } else if bytes.is_empty() {
                None
            } else {
                Some(Frame::Message(bytes))
            });
        }
        let newline = chunk.iter().position(|byte| *byte == b'\n');
        let content = newline.unwrap_or(chunk.len());
        if !oversized {
            if bytes.len() + content > MAX_MESSAGE {
                oversized = true;
                bytes.clear();
            } else {
                bytes.extend_from_slice(&chunk[..content]);
            }
        }
        reader.consume(content + usize::from(newline.is_some()));
        if newline.is_some() {
            return Ok(Some(if oversized {
                Frame::Oversized
            } else {
                Frame::Message(bytes)
            }));
        }
    }
}

#[cfg(test)]
mod tests;
