//! Minimal JMAP client (RFC 8620): session discovery, one authenticated POST per call,
//! errors split by whether the server may have acted.

use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use serde_json::{Value, json};
use std::time::Duration;

const MAIL: &str = "urn:ietf:params:jmap:mail";
const SUBMISSION: &str = "urn:ietf:params:jmap:submission";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The server said no (or the request never left): nothing was done.
    #[error("{kind}: {description}")]
    Rejected { kind: String, description: String },
    /// A timeout, reset or 5xx: the server may have acted and the answer was lost.
    #[error("outcome unknown: {0}")]
    Unknown(String),
    /// Bad input or a failed precondition caught here, before anything was sent.
    #[error("{0}")]
    Invalid(String),
}

impl Error {
    pub fn rejected(kind: impl Into<String>, description: impl Into<String>) -> Self {
        Error::Rejected {
            kind: kind.into(),
            description: description.into(),
        }
    }
    /// True when nothing can have happened on the server, so a retry is safe.
    pub fn is_definite(&self) -> bool {
        !matches!(self, Error::Unknown(_))
    }
}

pub type Result<T> = std::result::Result<T, Error>;

pub struct Jmap {
    http: reqwest::Client,
    api_url: String,
    pub account: String,
    /// Server limits from the session, used to chunk bulk calls.
    pub max_set: usize,
    pub max_get: usize,
}

impl Jmap {
    /// Fetches the session and checks the server can do mail and submission.
    pub async fn connect(base: &str, token: &str) -> Result<Self> {
        let base = base.trim_end_matches('/');
        if base.starts_with("http://") && !is_loopback(base) {
            return Err(Error::Invalid(
                "JMAP_URL is plain http on a non-loopback host; use https so the token is not sent in clear".into(),
            ));
        }
        let mut headers = HeaderMap::new();
        let mut auth = HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| {
            Error::Invalid("JMAP_TOKEN has characters that are not allowed in a header".into())
        })?;
        auth.set_sensitive(true);
        headers.insert(AUTHORIZATION, auth);
        let http = reqwest::Client::builder()
            .default_headers(headers)
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| Error::Invalid(e.to_string()))?;

        let res = http
            .get(format!("{base}/.well-known/jmap"))
            .send()
            .await
            .map_err(|e| {
                Error::rejected(
                    "connection",
                    format!("cannot fetch the JMAP session from {base}: {e}"),
                )
            })?;
        if !res.status().is_success() {
            let hint = if res.status().as_u16() == 401 {
                " (check JMAP_TOKEN)"
            } else {
                ""
            };
            return Err(Error::rejected(
                "session",
                format!("session request answered {}{hint}", res.status()),
            ));
        }
        let s: Value = res
            .json()
            .await
            .map_err(|e| Error::rejected("session", format!("bad session document: {e}")))?;
        let caps = &s["capabilities"];
        for c in [MAIL, SUBMISSION] {
            if caps.get(c).is_none() {
                return Err(Error::rejected(
                    "session",
                    format!("server does not offer {c}"),
                ));
            }
        }
        let account = s["primaryAccounts"][MAIL]
            .as_str()
            .ok_or_else(|| Error::rejected("session", "no primary mail account"))?
            .to_string();
        let api_url = s["apiUrl"]
            .as_str()
            .ok_or_else(|| Error::rejected("session", "no apiUrl"))?
            .to_string();
        let core = &caps["urn:ietf:params:jmap:core"];
        let lim = |k: &str| core[k].as_u64().unwrap_or(50).max(1) as usize;
        Ok(Jmap {
            http,
            api_url,
            account,
            max_set: lim("maxObjectsInSet"),
            max_get: lim("maxObjectsInGet"),
        })
    }

    /// A method call with `accountId` filled in.
    pub fn m(&self, name: &str, mut args: Value, id: &str) -> Value {
        args["accountId"] = json!(self.account);
        json!([name, args, id])
    }

    /// Sends one request. A method-level error response becomes `Error::Rejected`.
    pub async fn call(&self, calls: Vec<Value>) -> Result<Vec<Value>> {
        let resps = self.call_raw(calls).await?;
        for r in &resps {
            if r[0] == "error" {
                return Err(error_response(r));
            }
        }
        Ok(resps)
    }

    /// Like `call`, but `["error", ...]` responses stay in the list: a request with several
    /// methods can succeed in one and fail in the next, and a send must tell them apart.
    pub async fn call_raw(&self, calls: Vec<Value>) -> Result<Vec<Value>> {
        let body =
            json!({"using": ["urn:ietf:params:jmap:core", MAIL, SUBMISSION], "methodCalls": calls});
        let res = self
            .http
            .post(&self.api_url)
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                if e.is_connect() {
                    Error::rejected("connection", e.to_string())
                } else {
                    Error::Unknown(e.to_string())
                }
            })?;
        let status = res.status();
        if status.is_server_error() {
            return Err(Error::Unknown(format!("server answered {status}")));
        }
        if !status.is_success() {
            let hint = if status.as_u16() == 401 {
                " (check JMAP_TOKEN)"
            } else {
                ""
            };
            return Err(Error::rejected(
                "http",
                format!("server answered {status}{hint}"),
            ));
        }
        let v: Value = res
            .json()
            .await
            .map_err(|e| Error::Unknown(format!("unreadable answer: {e}")))?;
        v["methodResponses"]
            .as_array()
            .cloned()
            .ok_or_else(|| Error::Unknown("no methodResponses".into()))
    }

    /// Arguments of the first response with this method name and call id.
    pub fn find<'a>(resps: &'a [Value], name: &str, id: &str) -> Result<&'a Value> {
        resps
            .iter()
            .find(|r| r[0] == name && r[2] == id)
            .map(|r| &r[1])
            .ok_or_else(|| Error::Unknown(format!("answer has no {name} for {id}")))
    }
}

pub fn error_response(r: &Value) -> Error {
    Error::rejected(
        r[1]["type"].as_str().unwrap_or("serverFail"),
        r[1]["description"]
            .as_str()
            .unwrap_or("the server refused the request"),
    )
}

/// The first entry of `notCreated`, `notUpdated` or `notDestroyed` as an error.
pub fn set_failure(args: &Value, key: &str, id: &str) -> Option<Error> {
    let e = args.get(key)?.get(id)?;
    let kind = e["type"].as_str().unwrap_or("serverFail");
    let mut d = e["description"].as_str().unwrap_or("").to_string();
    if let Some(p) = e["properties"].as_array() {
        let p: Vec<_> = p.iter().filter_map(Value::as_str).collect();
        d = format!("{d} (properties: {})", p.join(", "))
            .trim()
            .to_string();
    }
    Some(Error::rejected(kind, d))
}

fn is_loopback(url: &str) -> bool {
    let rest = url.trim_start_matches("http://");
    rest.starts_with("[::1]")
        || matches!(
            rest.split(['/', ':']).next(),
            Some("localhost" | "127.0.0.1")
        )
}
