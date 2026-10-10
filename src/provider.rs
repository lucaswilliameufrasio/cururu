pub use cururu_engine::{ReviewUsage as ProviderUsage, merge_usage};

#[cfg(test)]
mod tests {
    use super::{ProviderUsage, merge_usage};
    use crate::agent::{ChunkResult, ReviewResult};
    use cururu_engine::ChatResponse;

    fn chunk(cost: Option<f64>) -> ChunkResult {
        ChunkResult {
            review: ReviewResult {
                model: "model".into(),
                files_reviewed: 1,
                summary: String::new(),
                findings: vec![],
            },
            usage: Some(ProviderUsage {
                prompt_tokens: 1,
                completion_tokens: 2,
                total_tokens: 3,
                cached_tokens: 0,
                reasoning_tokens: 0,
                cost,
            }),
        }
    }

    #[test]
    fn merged_cost_is_unavailable_if_any_chunk_lacks_provider_cost() {
        let merged = merge_usage(&[chunk(Some(0.25)), chunk(None)]).unwrap();
        assert_eq!(merged.prompt_tokens, 2);
        assert_eq!(merged.cost, None);
    }

    #[test]
    fn merged_cost_sums_provider_costs_when_every_chunk_reports_them() {
        let merged = merge_usage(&[chunk(Some(0.25)), chunk(Some(0.5))]).unwrap();
        assert_eq!(merged.cost, Some(0.75));
    }

    #[test]
    fn reports_provider_error_when_response_has_no_choices() {
        let response: ChatResponse = serde_json::from_str(
            r#"{"error":{"message":"No endpoints found for this model"},"request_id":"req-123"}"#,
        )
        .unwrap();

        let error = response
            .first_choice("LLM returned no choices")
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("No endpoints found for this model")
        );
    }

    #[test]
    fn explains_unexpected_response_shape_when_choices_are_missing() {
        let response: ChatResponse = serde_json::from_str(r#"{"status":"ok"}"#).unwrap();

        let error = response
            .first_choice("LLM returned no choices")
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("OpenAI-compatible `choices` array")
        );
    }
}
