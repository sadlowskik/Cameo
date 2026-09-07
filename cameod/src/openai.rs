//! Published OpenAI feature matrix for the `/v1` gateway.
//!
//! Cameo advertises a precise subset. Unsupported parameters are rejected
//! here so a backend cannot ignore them and return a plausible answer.

use serde_json::{json, Value};

use crate::http::Response;

pub enum GatewayRoute {
    ChatCompletions,
    Completions,
    Embeddings,
}

pub fn route_from_path(rest: &[&str]) -> Option<GatewayRoute> {
    match rest {
        ["chat", "completions"] => Some(GatewayRoute::ChatCompletions),
        ["completions"] => Some(GatewayRoute::Completions),
        ["embeddings"] => Some(GatewayRoute::Embeddings),
        _ => None,
    }
}

/// Reject parameters this node does not implement. `None` means the body may
/// continue to model routing.
pub fn reject_unsupported(route: GatewayRoute, body: &Value) -> Option<Response> {
    if body.get("tools").is_some()
        || body.get("functions").is_some()
        || tool_choice_requests_tools(body)
    {
        return Some(unsupported(
            "native tool calls are not available; the agent harness owns tool orchestration",
            "tools",
        ));
    }
    if body.get("logprobs").is_some() || body.get("top_logprobs").is_some() {
        return Some(unsupported(
            "logprobs are not advertised by this node",
            "logprobs",
        ));
    }
    if body.get("logit_bias").is_some() {
        return Some(unsupported(
            "logit_bias is not advertised by this node",
            "logit_bias",
        ));
    }
    if body
        .get("n")
        .and_then(Value::as_u64)
        .is_some_and(|n| n != 1)
    {
        return Some(unsupported("only n=1 completions are advertised", "n"));
    }
    if multimodal(body) {
        return Some(unsupported(
            "image and multimodal inputs are not advertised by this node",
            "messages",
        ));
    }
    if matches!(
        body.pointer("/response_format/type")
            .and_then(Value::as_str),
        Some("json_schema")
    ) {
        return Some(unsupported(
            "structured json_schema response_format is not advertised",
            "response_format",
        ));
    }
    match route {
        GatewayRoute::ChatCompletions | GatewayRoute::Completions => {
            if body.get("best_of").is_some() || body.get("echo").is_some() {
                return Some(unsupported(
                    "best_of and echo are not advertised by this node",
                    "best_of",
                ));
            }
        }
        GatewayRoute::Embeddings => {
            if body
                .get("encoding_format")
                .and_then(Value::as_str)
                .is_some_and(|format| format != "float")
            {
                return Some(unsupported(
                    "only float embedding encoding is advertised",
                    "encoding_format",
                ));
            }
        }
    }
    None
}

pub fn reject_token_ceiling(body: &Value, context_tokens: Option<u32>) -> Option<Response> {
    let context = context_tokens?;
    let requested = body
        .get("max_completion_tokens")
        .or_else(|| body.get("max_tokens"))
        .and_then(Value::as_u64)?;
    if requested > u64::from(context) {
        return Some(unsupported(
            format!("max_tokens {requested} exceeds the served context window {context}"),
            "max_tokens",
        ));
    }
    None
}

fn tool_choice_requests_tools(body: &Value) -> bool {
    match body.get("tool_choice") {
        None | Some(Value::Null) => false,
        Some(Value::String(choice)) => choice != "none",
        Some(_) => true,
    }
}

fn multimodal(body: &Value) -> bool {
    body.get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .any(|message| match message.get("content") {
            Some(Value::Array(parts)) => parts.iter().any(|part| {
                part.get("type").and_then(Value::as_str) == Some("image_url")
                    || part.get("image_url").is_some()
            }),
            _ => false,
        })
}

fn unsupported(message: impl Into<String>, param: &str) -> Response {
    Response::json(
        400,
        &json!({
            "error": {
                "message": message.into(),
                "type": "invalid_request_error",
                "code": "unsupported_parameter",
                "param": param,
            }
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advertised_chat_body_is_accepted() {
        let body = json!({"model":"fixture","messages":[{"role":"user","content":"hi"}],"stream":true,"n":1});
        assert!(reject_unsupported(GatewayRoute::ChatCompletions, &body).is_none());
    }

    #[test]
    fn native_tools_and_images_and_logprobs_are_rejected() {
        for body in [
            json!({"model":"x","tools":[]}),
            json!({"model":"x","tool_choice":"auto"}),
            json!({"model":"x","logprobs":true}),
            json!({"model":"x","n":2}),
            json!({"model":"x","messages":[{"role":"user","content":[{"type":"image_url","image_url":{"url":"x"}}]}]}),
            json!({"model":"x","response_format":{"type":"json_schema","json_schema":{}}}),
        ] {
            let response = reject_unsupported(GatewayRoute::ChatCompletions, &body).unwrap();
            assert_eq!(response.status, 400);
            let parsed: Value = serde_json::from_slice(&response.body).unwrap();
            assert_eq!(parsed["error"]["code"], "unsupported_parameter");
        }
        assert!(reject_unsupported(
            GatewayRoute::ChatCompletions,
            &json!({"model":"x","tool_choice":"none"})
        )
        .is_none());
    }

    #[test]
    fn completion_token_ceiling_uses_served_context() {
        let body = json!({"max_tokens": 4097});
        assert!(reject_token_ceiling(&body, None).is_none());
        assert!(reject_token_ceiling(&body, Some(4096)).is_some());
        assert!(reject_token_ceiling(&json!({"max_tokens": 4096}), Some(4096)).is_none());
    }
}
