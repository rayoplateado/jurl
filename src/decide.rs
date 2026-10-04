//! Clients for System One–style decision models. Jev (TypeSafe) and Clef
//! (Cloudflare Workers AI) share the `{state, questions} → {answers}` contract.

use std::{collections::HashMap, time::Duration};

use anyhow::{Result, bail};
use reqwest::{Client, StatusCode};
use serde_json::{Map, Value, json};

pub const JEV_MODEL: &str = "jev-1.13.0";
pub const CLEF_MODEL: &str = "clef-flash";

#[derive(Debug, Default)]
pub struct Answers {
    pub answers: HashMap<String, Value>,
    pub input_tokens: u64,
    pub requests: usize,
}

impl Answers {
    pub fn label(&self) -> String {
        format!("jev({} req, {} tok)", self.requests, self.input_tokens)
    }

    pub fn noul(&self, id: &str) -> Option<f64> {
        self.answers.get(id)?.get("noul")?.as_f64()
    }

    pub fn probabilities(&self, id: &str) -> Option<HashMap<String, f64>> {
        let p = self.answers.get(id)?.get("probabilities")?.as_object()?;
        Some(p.iter().filter_map(|(k, v)| Some((k.clone(), v.as_f64()?))).collect())
    }

    pub fn choice(&self, id: &str) -> Option<(String, f64)> {
        let a = self.answers.get(id)?;
        Some((a.get("choice")?.as_str()?.to_string(), a.get("confidence").and_then(Value::as_f64).unwrap_or(0.0)))
    }
}

pub fn noul(instructions: impl Into<String>) -> Value {
    json!({ "type": "noul", "instructions": instructions.into() })
}

pub fn choice(instructions: &str, criteria: Value) -> Value {
    json!({ "type": "choice", "instructions": instructions, "criteria": criteria })
}

pub async fn jev(client: &Client, key: &str, state: Value, questions: Map<String, Value>) -> Result<Answers> {
    let body = json!({ "state": state, "model": JEV_MODEL, "questions": questions });
    let v = post(client, "https://api.typesafe.ai/v1/systemone", key, &body).await?;
    Ok(parse(&v))
}

pub async fn clef(
    client: &Client,
    account: &str,
    token: &str,
    state: Value,
    questions: Map<String, Value>,
    images: Vec<String>,
) -> Result<Answers> {
    let url = format!("https://api.cloudflare.com/client/v4/accounts/{account}/ai/run/@cf/cloudflare/{CLEF_MODEL}");
    let body = json!({ "model": CLEF_MODEL, "state": state, "questions": questions, "images": images });
    let v = post(client, &url, token, &body).await?;
    Ok(parse(v.get("result").unwrap_or(&v)))
}

fn parse(v: &Value) -> Answers {
    let answers = v
        .get("answers")
        .and_then(Value::as_object)
        .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default();
    let input_tokens = v.pointer("/usage/input_tokens").and_then(Value::as_u64).unwrap_or(0);
    Answers { answers, input_tokens, requests: 1 }
}

/// POST with a short backoff on 429/529, honouring `Retry-After`.
async fn post(client: &Client, url: &str, bearer: &str, body: &Value) -> Result<Value> {
    let mut attempt = 0;
    loop {
        let res = client.post(url).bearer_auth(bearer).json(body).send().await?;
        let status = res.status();
        if status.is_success() {
            return Ok(res.json().await?);
        }
        let retryable = status == StatusCode::TOO_MANY_REQUESTS || status.as_u16() == 529;
        if retryable && attempt < 2 {
            let wait = res
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok()?.parse::<f64>().ok())
                .map_or(Duration::from_millis(300 << attempt), Duration::from_secs_f64);
            tokio::time::sleep(wait.min(Duration::from_secs(5))).await;
            attempt += 1;
            continue;
        }
        let text = res.text().await.unwrap_or_default();
        bail!("{url} → HTTP {status}: {}", text.chars().take(400).collect::<String>());
    }
}
