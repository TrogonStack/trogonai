//! Minimal gateway-hosted consent screen.
//!
//! When a `ConsentPolicy::decide` returns `Interaction`, the gateway needs
//! to show the user a page that lists the requested scopes and lets them
//! Allow/Deny. We render a small self-contained HTML form to avoid pulling
//! in a templating engine. The form posts back to the same gateway under
//! `/aauth/oauth/consent` with the original interaction `code` and the
//! user's decision; the handler then resumes the Authorization Code flow.

use serde::{Deserialize, Serialize};

use crate::oauth_issuer_config::ConsentBranding;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsentScreenContext {
    pub client_id: String,
    pub principal: String,
    pub scopes: Vec<String>,
    pub interaction_code: String,
    pub state: Option<String>,
    pub redirect_uri: String,
    #[serde(default)]
    pub branding: ConsentBranding,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsentSubmission {
    pub interaction_code: String,
    pub decision: String,
    #[serde(default)]
    pub state: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsentChoice {
    Allow,
    Deny,
}

impl ConsentSubmission {
    pub fn choice(&self) -> Option<ConsentChoice> {
        match self.decision.as_str() {
            "allow" => Some(ConsentChoice::Allow),
            "deny" => Some(ConsentChoice::Deny),
            _ => None,
        }
    }
}

pub fn render_html(ctx: &ConsentScreenContext) -> String {
    let scope_items = ctx
        .scopes
        .iter()
        .map(|s| format!("<li><code>{}</code></li>", html_escape(s)))
        .collect::<String>();
    let state = ctx
        .state
        .as_deref()
        .map(|s| format!(r#"<input type="hidden" name="state" value="{}">"#, html_escape(s)))
        .unwrap_or_default();
    let platform_header = if ctx.branding.platform_name.is_empty() {
        String::new()
    } else {
        format!(
            r#"<header class="platform"><strong>{}</strong></header>"#,
            html_escape(&ctx.branding.platform_name)
        )
    };
    let logo = ctx
        .branding
        .logo_url
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(|url| {
            format!(
                r#"<img src="{}" alt="{}" class="logo">"#,
                html_escape(url),
                html_escape(&ctx.branding.platform_name)
            )
        })
        .unwrap_or_default();
    let legal = ctx
        .branding
        .legal_text
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(|t| format!(r#"<p class="legal">{}</p>"#, html_escape(t)))
        .unwrap_or_default();
    format!(
        r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><title>Authorize {client}</title>
<style>body{{font-family:system-ui,sans-serif;max-width:32rem;margin:4rem auto;padding:0 1rem}}
button{{font-size:1rem;padding:.5rem 1rem;margin-right:.5rem}}
.logo{{max-height:3rem;margin-bottom:1rem}}
.legal{{color:#555;font-size:.85rem;border-top:1px solid #eee;padding-top:1rem;margin-top:2rem}}</style></head><body>
{logo}{platform_header}<h1>Authorize <code>{client}</code></h1>
<p>Hi <strong>{principal}</strong>, the application <code>{client}</code> is requesting access:</p>
<ul>{scopes}</ul>
<p>Continuing will redirect you to <code>{redirect}</code>.</p>
<form method="post" action="/aauth/oauth/consent">
<input type="hidden" name="interaction_code" value="{code}">
{state}
<button name="decision" value="allow" type="submit">Allow</button>
<button name="decision" value="deny" type="submit" formnovalidate>Deny</button>
</form>
{legal}
</body></html>"#,
        client = html_escape(&ctx.client_id),
        principal = html_escape(&ctx.principal),
        scopes = scope_items,
        redirect = html_escape(&ctx.redirect_uri),
        code = html_escape(&ctx.interaction_code),
        state = state,
        logo = logo,
        platform_header = platform_header,
        legal = legal,
    )
}

fn html_escape(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> ConsentScreenContext {
        ConsentScreenContext {
            client_id: "cursor-cli".into(),
            principal: "alice@example.com".into(),
            scopes: vec!["read:tools".into(), "write:tools".into()],
            interaction_code: "icode-1".into(),
            state: Some("xyz".into()),
            redirect_uri: "http://127.0.0.1:8765/callback".into(),
            branding: crate::oauth_issuer_config::ConsentBranding::default(),
        }
    }

    #[test]
    fn html_renders_each_scope_and_hidden_state() {
        let html = render_html(&ctx());
        assert!(html.contains("<code>read:tools</code>"));
        assert!(html.contains("<code>write:tools</code>"));
        assert!(html.contains(r#"name="state" value="xyz""#));
        assert!(html.contains(r#"name="interaction_code" value="icode-1""#));
        assert!(html.contains(r#"name="decision" value="allow""#));
        assert!(html.contains(r#"name="decision" value="deny""#));
    }

    #[test]
    fn html_escapes_user_supplied_strings() {
        let mut c = ctx();
        c.client_id = "<script>alert(1)</script>".into();
        let html = render_html(&c);
        assert!(!html.contains("<script>alert(1)</script>"));
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
    }

    #[test]
    fn branding_block_renders_platform_logo_and_legal() {
        let mut c = ctx();
        c.branding = crate::oauth_issuer_config::ConsentBranding {
            platform_name: "Acme Platform".into(),
            logo_url: Some("https://cdn.example.com/logo.png".into()),
            legal_text: Some("Terms apply".into()),
        };
        let html = render_html(&c);
        assert!(html.contains("Acme Platform"));
        assert!(html.contains(r#"src="https://cdn.example.com/logo.png""#));
        assert!(html.contains("Terms apply"));
    }

    #[test]
    fn submission_parses_allow_and_deny() {
        let allow = ConsentSubmission {
            interaction_code: "i".into(),
            decision: "allow".into(),
            state: None,
        };
        let deny = ConsentSubmission {
            interaction_code: "i".into(),
            decision: "deny".into(),
            state: None,
        };
        let other = ConsentSubmission {
            interaction_code: "i".into(),
            decision: "what".into(),
            state: None,
        };
        assert_eq!(allow.choice(), Some(ConsentChoice::Allow));
        assert_eq!(deny.choice(), Some(ConsentChoice::Deny));
        assert_eq!(other.choice(), None);
    }
}
