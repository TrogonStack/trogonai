use super::*;

fn host() -> Result<Host, Box<dyn std::error::Error>> {
    Ok(Host::new("api.example.test")?)
}

fn headers(list: &[(&str, &str)]) -> Vec<(String, String)> {
    list.iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect()
}

fn arc(mutator: impl HeaderMutator + 'static) -> Arc<dyn HeaderMutator> {
    Arc::new(mutator)
}

#[test]
fn a_mutator_edits_headers_for_the_destination_it_is_given() -> Result<(), Box<dyn std::error::Error>> {
    let mutator = arc(|headers: &mut Vec<(String, String)>, host: &Host| {
        headers.push(("x-dest".to_string(), host.to_string()));
    });
    let out = mutated(&mutator, &headers(&[("accept", "*/*")]), &host()?)?;
    assert_eq!(out, headers(&[("accept", "*/*"), ("x-dest", "api.example.test")]));
    Ok(())
}

#[test]
fn a_panic_a_malformed_header_or_a_framing_change_is_refused() -> Result<(), Box<dyn std::error::Error>> {
    let panics = arc(|_: &mut Vec<(String, String)>, _: &Host| std::panic::resume_unwind(Box::new("boom")));
    let broken = arc(|headers: &mut Vec<(String, String)>, _: &Host| {
        headers.push(("x-bad".to_string(), "a\r\nb".to_string()));
    });
    let reframes = arc(|headers: &mut Vec<(String, String)>, _: &Host| {
        headers.retain(|(name, _)| name != "content-length");
    });
    let input = headers(&[("content-length", "3")]);
    for mutator in [panics, broken, reframes] {
        assert_eq!(mutated(&mutator, &input, &host()?), Err(MutationRefusedError));
    }
    Ok(())
}
