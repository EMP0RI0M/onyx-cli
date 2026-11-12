pub mod protocol;
pub mod provider;
pub mod router;
pub mod server;

pub use protocol::{ChatCompletionRequest, ChatCompletionResponse, ChatMessage};
pub use provider::{OpenAiCompatClient, ProviderClient};
pub use router::{RouteDecision, Router, UpstreamTarget};
pub use server::OmniServer;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    struct MockProvider;
    impl ProviderClient for MockProvider {
        fn id(&self) -> &str { "mock" }
        fn forward(&self, req: &ChatCompletionRequest) -> Result<ChatCompletionResponse, String> {
            Ok(ChatCompletionResponse {
                id: "test-id".into(),
                object: "chat.completion".into(),
                created: 1700000000,
                model: req.model.clone(),
                choices: vec![protocol::ChatChoice {
                    index: 0,
                    message: ChatMessage {
                        role: "assistant".into(),
                        content: "pong".into(),
                    },
                    finish_reason: Some("stop".into()),
                }],
            })
        }
    }

    #[test]
    fn test_router_fallback() {
        let mut r = Router::new();
        r.add_target(UpstreamTarget {
            provider: Arc::new(MockProvider),
            mapped_model: "fallback-model".into(),
        });

        let req = ChatCompletionRequest {
            model: "unknown-model".into(),
            messages: vec![ChatMessage {
                role: "user".into(),
                content: "hi".into(),
            }],
            temperature: None,
            max_tokens: None,
            stream: None,
        };

        let decision = r.route(&req).unwrap();
        assert_eq!(decision.target.mapped_model, "fallback-model");
        let resp = decision.target.provider.forward(&req).unwrap();
        assert_eq!(resp.choices[0].message.content, "pong");
    }
}
