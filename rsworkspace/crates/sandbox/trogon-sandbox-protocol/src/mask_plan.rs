use std::ops::Range;

use trogon_sandbox_contract::MaskDuplicates;

use crate::Sentinel;

struct Placement {
    span: Range<usize>,
    capture: usize,
}

/// The distinct values found in a file, in first-seen order, and where each one is to be replaced.
///
/// Spans are computed against the original content, so a sentinel inserted by [`MaskPlan::render`] is never matched
/// again. Located spans win over verbatim copies, and copies are scanned longest value first so a value that is a
/// substring of a longer one cannot take part of it.
pub struct MaskPlan<'a> {
    content: &'a str,
    captures: Vec<&'a str>,
    placements: Vec<Placement>,
}

impl<'a> MaskPlan<'a> {
    pub(crate) fn new(content: &'a str, spans: Vec<Range<usize>>, mask_duplicates: MaskDuplicates) -> Self {
        let mut captures: Vec<&'a str> = Vec::new();
        let mut placements = Vec::with_capacity(spans.len());
        for span in spans {
            let value = &content[span.clone()];
            let capture = captures.iter().position(|seen| *seen == value).unwrap_or_else(|| {
                captures.push(value);
                captures.len() - 1
            });
            placements.push(Placement { span, capture });
        }
        if mask_duplicates.is_enabled() {
            let mut occupied = vec![false; content.len()];
            for placement in &placements {
                occupied[placement.span.clone()].fill(true);
            }
            let mut by_length: Vec<usize> = (0..captures.len()).collect();
            by_length.sort_by_key(|index| std::cmp::Reverse(captures[*index].len()));
            for capture in by_length {
                let value = captures[capture];
                let mut from = 0;
                while let Some(found) = content[from..].find(value) {
                    let start = from + found;
                    let end = start + value.len();
                    if !occupied[start..end].iter().any(|taken| *taken) {
                        occupied[start..end].fill(true);
                        placements.push(Placement {
                            span: start..end,
                            capture,
                        });
                    }
                    from = start + content[start..].chars().next().map_or(1, char::len_utf8);
                }
            }
            placements.sort_by_key(|placement| placement.span.start);
        }
        Self {
            content,
            captures,
            placements,
        }
    }

    /// The distinct captured values in first-seen order.
    pub fn captures(&self) -> &[&'a str] {
        &self.captures
    }

    /// The content with each placement replaced by the sentinel of its capture.
    ///
    /// `sentinels` holds one per capture, in [`Self::captures`] order; a shorter slice is a caller bug and panics on
    /// the first placement it does not cover.
    pub fn render(&self, sentinels: &[Sentinel]) -> String {
        let mut out = String::with_capacity(self.content.len());
        let mut pos = 0;
        for placement in &self.placements {
            out.push_str(&self.content[pos..placement.span.start]);
            out.push_str(sentinels[placement.capture].as_str());
            pos = placement.span.end;
        }
        out.push_str(&self.content[pos..]);
        out
    }
}
