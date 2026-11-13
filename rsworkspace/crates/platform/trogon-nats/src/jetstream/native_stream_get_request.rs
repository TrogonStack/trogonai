#[derive(serde::Serialize)]
#[serde(untagged)]
pub(crate) enum NativeStreamGetRequest<'a> {
    Sequence { seq: u64 },
    LastBySubject { last_by_subj: &'a str },
    NextBySubject { seq: u64, next_by_subj: &'a str },
}
