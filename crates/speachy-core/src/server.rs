//! Models that run in a speech server (see [`crate::catalog::Server`]).
//!
//! The server is asked the way OpenAI's speech API is: a `POST` to
//! `/v1/audio/speech` with the model, the text and the voice, answered with
//! the audio. mlx-audio, which runs most open models on Apple Silicon,
//! speaks it, as do many other local servers. Speachy asks for one chunk of
//! text at a time, so everything the pipeline does — starting early,
//! preparing ahead, playing without gaps — works the same as for a model
//! that runs inside it.

use crate::catalog::{Catalog, Server};
use crate::engine::{Audio, SynthRequest, Synthesizer};
use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;
use serde_json::{json, Value};
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

/// How long a server may take over one answer. Generous: the first request
/// for a model can make the server download and load it.
const ANSWER_WITHIN: Duration = Duration::from_secs(900);

fn agent(answer_within: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(5)))
        .timeout_recv_response(Some(answer_within))
        // mlx-audio says "200 OK" at once and sends the audio when it is
        // done, so the wait for the audio is as long as the wait for the
        // answer.
        .timeout_per_read(Some(answer_within))
        .user_agent(concat!("speachy/", env!("CARGO_PKG_VERSION")))
        // Read the server's own explanation instead of a bare status.
        .http_status_as_error(false)
        .build()
        .into()
}

/// What a server said went wrong, shortened to something that fits a
/// notice.
fn complaint(status: u16, body: &[u8]) -> String {
    let text = String::from_utf8_lossy(body);
    // FastAPI and OpenAI both wrap the message in JSON.
    let message = serde_json::from_str::<Value>(&text)
        .ok()
        .and_then(|value| {
            let inner = value.get("detail").or_else(|| value.get("error"))?;
            let inner = inner.get("message").unwrap_or(inner);
            Some(match inner {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            })
        })
        .unwrap_or_else(|| text.trim().to_string());
    let short: String = message.chars().take(300).collect();
    if short.is_empty() {
        format!("the speech server answered {status}")
    } else {
        format!("the speech server answered {status}: {short}")
    }
}

fn unreachable(endpoint: &str, err: &ureq::Error) -> anyhow::Error {
    anyhow!("the speech server at {endpoint} is not answering ({err}). Is it running?")
}

/// A model in a speech server.
pub struct ServerEngine {
    model_id: String,
    server: Server,
    catalog: Arc<Catalog>,
    agent: ureq::Agent,
}

impl ServerEngine {
    pub fn new(model_id: &str, server: &Server, catalog: Arc<Catalog>) -> Self {
        Self {
            model_id: model_id.to_string(),
            server: server.clone(),
            catalog,
            agent: agent(ANSWER_WITHIN),
        }
    }

    /// The request for `req`, as the server expects it.
    fn request(&self, req: &SynthRequest) -> Value {
        let server = &self.server;
        // The user's own fields first: they do not replace what follows.
        let mut body = server.options.clone();
        body.insert("model".into(), json!(server.model));
        body.insert("input".into(), json!(req.text));
        body.insert("response_format".into(), json!("wav"));
        body.insert("speed".into(), json!(req.speed.clamp(0.25, 4.0)));
        if let Some(param) = &server.language_param {
            let value = server
                .language_values
                .get(req.lang)
                .map_or(req.lang, String::as_str);
            body.insert(param.clone(), json!(value));
        }
        // Looked up now, not when the model was loaded, so a voice the user
        // has just added works.
        let model = self.catalog.find(&self.model_id);
        let voice = model.as_ref().and_then(|m| m.voice(req.voice));
        match voice.and_then(|v| v.reference.as_ref()) {
            // A voice of the user's: the server imitates the recording.
            Some(recording) => {
                body.insert("ref_audio".into(), json!(recording));
                if let Some(text) = voice.and_then(|v| v.reference_text.as_ref()) {
                    body.insert("ref_text".into(), json!(text));
                }
            }
            None => {
                body.insert("voice".into(), json!(req.voice));
            }
        }
        Value::Object(body)
    }
}

/// Sends `body` and returns the audio the server answers with.
fn ask(agent: &ureq::Agent, endpoint: &str, body: &Value, cancel: &AtomicBool) -> Result<Audio> {
    let url = format!("{endpoint}/v1/audio/speech");
    let response = agent
        .post(&url)
        .header("Content-Type", "application/json")
        .send(body.to_string())
        .map_err(|err| unreachable(endpoint, &err))?;
    let status = response.status().as_u16();
    let mut reader = response.into_body().into_reader();
    let mut bytes = Vec::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        if cancel.load(Ordering::Relaxed) {
            bail!("cancelled");
        }
        // A server that fails while it speaks has already said "200 OK":
        // all it can do then is break off.
        let n = reader.read(&mut buf).map_err(|err| match err.kind() {
            std::io::ErrorKind::TimedOut => anyhow!(
                "the speech server did not finish within {} minutes",
                ANSWER_WITHIN.as_secs() / 60
            ),
            _ => {
                anyhow!("the speech server broke off its answer ({err}); its own log will say why")
            }
        })?;
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&buf[..n]);
        if bytes.len() > 512 * 1024 * 1024 {
            bail!("the speech server sent more than 512 MB for one chunk of text");
        }
    }
    if !(200..300).contains(&status) {
        bail!(complaint(status, &bytes));
    }
    if bytes.is_empty() {
        bail!("the speech server answered without any audio; its own log will say why");
    }
    crate::wav::decode(&bytes).context("the speech server did not answer with WAV audio")
}

impl Synthesizer for ServerEngine {
    fn synthesize(&self, req: &SynthRequest, cancel: &Arc<AtomicBool>) -> Result<Option<Audio>> {
        let body = self.request(req);
        // Asked on a thread of its own: a request cannot be taken back once
        // it is sent, and stopping must not wait for a slow server.
        let (tx, rx) = mpsc::channel();
        let (agent, endpoint) = (self.agent.clone(), self.server.endpoint.clone());
        let stop = cancel.clone();
        std::thread::Builder::new()
            .name("speachy-server-request".into())
            .spawn(move || {
                let _ = tx.send(ask(&agent, &endpoint, &body, &stop));
            })
            .context("could not start a thread for the request")?;
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Ok(None);
            }
            match rx.recv_timeout(Duration::from_millis(40)) {
                Ok(result) => {
                    return match result {
                        Ok(audio) => Ok(Some(audio)),
                        Err(_) if cancel.load(Ordering::Relaxed) => Ok(None),
                        Err(err) => Err(err),
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    bail!("the request to the speech server was lost")
                }
            }
        }
    }
}

/// What a speech server says it offers.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Offer {
    /// Models, as the server names them.
    pub models: Vec<String>,
    /// Voices of the model asked about; empty when the server does not say.
    pub voices: Vec<String>,
}

fn percent_encoded(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// The names in a list a server answered with. Servers agree on little:
/// the list sits under `data`, `voices` or `models`, or is the answer
/// itself, and holds either names or objects with an `id` or `name`.
fn names(answer: &Value) -> Vec<String> {
    let list = ["data", "voices", "models"]
        .iter()
        .find_map(|key| answer.get(key).and_then(Value::as_array))
        .or_else(|| answer.as_array());
    list.into_iter()
        .flatten()
        .filter_map(|item| match item {
            Value::String(name) => Some(name.clone()),
            other => ["id", "name", "voice_id"]
                .iter()
                .find_map(|key| other.get(key).and_then(Value::as_str))
                .map(str::to_string),
        })
        .collect()
}

/// Asks the server at `endpoint` which models it has and, for `model`,
/// which voices. Fails if the server does not answer; a server that
/// answers but does not list models or voices offers nothing.
pub fn offer(endpoint: &str, model: Option<&str>) -> Result<Offer> {
    let endpoint = endpoint.trim().trim_end_matches('/');
    if !endpoint.starts_with("http://") && !endpoint.starts_with("https://") {
        bail!("the server address must start with http:// or https://");
    }
    let agent = agent(Duration::from_secs(20));
    let get = |path: &str| -> Result<Option<Value>> {
        let response = agent
            .get(format!("{endpoint}{path}"))
            .call()
            .map_err(|err| unreachable(endpoint, &err))?;
        if !response.status().is_success() {
            return Ok(None);
        }
        let mut text = String::new();
        response
            .into_body()
            .into_reader()
            .take(4 * 1024 * 1024)
            .read_to_string(&mut text)?;
        Ok(serde_json::from_str(&text).ok())
    };
    let models = get("/v1/models")?.as_ref().map(names).unwrap_or_default();
    let voices = match model {
        Some(model) => get(&format!(
            "/v1/audio/voices?model={}",
            percent_encoded(model)
        ))?,
        None => get("/v1/audio/voices")?,
    };
    Ok(Offer {
        models,
        voices: voices.as_ref().map(names).unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_lists_servers_answer_with() {
        // mlx-audio and OpenAI: objects under "data".
        let answer = json!({ "object": "list", "data": [{ "id": "a/b", "object": "model" }, { "id": "c" }] });
        assert_eq!(names(&answer), ["a/b", "c"]);
        // Plain names under "voices", and a bare list of objects.
        assert_eq!(
            names(&json!({ "voices": ["af_heart", "bm_george"] })),
            ["af_heart", "bm_george"]
        );
        assert_eq!(
            names(&json!([{ "name": "Anna" }, { "voice_id": "ivan" }])),
            ["Anna", "ivan"]
        );
        assert!(names(&json!({ "message": "hello" })).is_empty());
        assert!(names(&json!("text")).is_empty());
    }

    #[test]
    fn explains_what_the_server_said() {
        let fastapi = br#"{"detail":"Reference audio file not found: /x.wav"}"#;
        assert_eq!(
            complaint(400, fastapi),
            "the speech server answered 400: Reference audio file not found: /x.wav"
        );
        let openai = br#"{"error":{"message":"model not found","type":"invalid_request_error"}}"#;
        assert_eq!(
            complaint(404, openai),
            "the speech server answered 404: model not found"
        );
        assert_eq!(
            complaint(500, b"  boom\n"),
            "the speech server answered 500: boom"
        );
        assert_eq!(complaint(502, b""), "the speech server answered 502");
        assert!(complaint(500, "x".repeat(5000).as_bytes()).len() < 400);
    }

    #[test]
    fn model_names_survive_a_query_string() {
        assert_eq!(
            percent_encoded("mlx-community/Kokoro-82M bf16"),
            "mlx-community%2FKokoro-82M%20bf16"
        );
    }
}
