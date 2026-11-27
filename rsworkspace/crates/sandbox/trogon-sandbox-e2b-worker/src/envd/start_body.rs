use std::collections::BTreeMap;

use serde::Serialize;

use super::constants::{BASH_PREFIX_ARGS, BASH_PROGRAM};
use super::{ProcessRequest, PtyRequest};

/// Body of `process.Process/Start`.
#[derive(Serialize)]
pub(super) struct StartBody<'a> {
    process: ProcessBody<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pty: Option<PtyBody>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tag: Option<&'a str>,
    stdin: bool,
}

#[derive(Serialize)]
struct ProcessBody<'a> {
    cmd: &'a str,
    args: Vec<&'a str>,
    envs: &'a BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cwd: Option<&'a str>,
}

#[derive(Serialize)]
pub(super) struct PtyBody {
    pub(super) size: PtySizeBody,
}

#[derive(Serialize)]
pub(super) struct PtySizeBody {
    pub(super) cols: u16,
    pub(super) rows: u16,
}

impl<'a> StartBody<'a> {
    /// `argv` run through `bash -l -c 'exec "$@"'`, which keeps it exact.
    pub(super) fn command(request: &'a ProcessRequest) -> Self {
        let args = BASH_PREFIX_ARGS
            .into_iter()
            .chain(request.argv.iter().map(String::as_str))
            .collect();
        Self {
            process: ProcessBody {
                cmd: BASH_PROGRAM,
                args,
                envs: &request.env,
                cwd: request.cwd.as_deref(),
            },
            pty: None,
            tag: None,
            stdin: false,
        }
    }

    pub(super) fn pty(request: &'a PtyRequest) -> Self {
        Self {
            process: ProcessBody {
                cmd: &request.cmd,
                args: request.args.iter().map(String::as_str).collect(),
                envs: &request.env,
                cwd: None,
            },
            pty: Some(PtyBody {
                size: PtySizeBody {
                    cols: request.cols,
                    rows: request.rows,
                },
            }),
            tag: Some(&request.tag),
            stdin: true,
        }
    }
}
