use uuid::Uuid;

/// A `multipart/form-data` body that uploads one file: the boundary to name in the content type, and the body.
pub(crate) fn file_part(path: &str, content: &[u8]) -> (String, Vec<u8>) {
    let boundary = format!("trogon-{}", Uuid::new_v4().simple());
    let quoted = path
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace(['\r', '\n'], "");
    let mut body = Vec::with_capacity(content.len() + 256);
    body.extend_from_slice(
        format!("--{boundary}\r\nContent-Disposition: form-data; name=\"files[0].path\"\r\n\r\n{path}\r\n").as_bytes(),
    );
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"files[0].file\"; filename=\"{quoted}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(content);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (boundary, body)
}

#[cfg(test)]
mod tests;
