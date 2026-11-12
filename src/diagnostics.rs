//! Deterministic, localized diagnostics for reviews that could not complete.
//! They name the setting to change, its current value and a suggestion,
//! without another model call.

use crate::config::AppConfig;
use std::fmt::Write as _;

/// The largest output limit Cururu suggests on its own.
pub const MAX_SUGGESTED_OUTPUT_TOKENS: u32 = 32_000;

pub struct Diagnostic {
    pub title: &'static str,
    pub body: String,
}

/// Explain an incomplete model response for `finish_reason`.
pub fn incomplete_review(config: &AppConfig, finish_reason: &str) -> Diagnostic {
    let pt = config
        .review
        .language
        .to_ascii_lowercase()
        .starts_with("pt");
    let current = config.llm.max_output_tokens;
    let suggested = current.saturating_mul(2).min(MAX_SUGGESTED_OUTPUT_TOKENS);
    let chunk = config.review.chunk_bytes;
    let mut body = String::new();
    let _ = writeln!(
        body,
        "{}\n",
        if pt {
            format!(
                "O modelo não concluiu o review (finish reason: `{finish_reason}`). Nada foi preparado nem publicado."
            )
        } else {
            format!(
                "The model did not complete the review (finish reason: `{finish_reason}`). Nothing was prepared or published."
            )
        }
    );
    match finish_reason {
        "length" | "max_tokens" => {
            if current < MAX_SUGGESTED_OUTPUT_TOKENS {
                let _ = writeln!(
                    body,
                    "{}\n\n```toml\n[provider]\nmax_output_tokens = {suggested}\n```\n",
                    if pt {
                        format!(
                            "A resposta atingiu o limite de saída. `provider.max_output_tokens` está em **{current}**; aumente-o no `.cururu.toml` do repositório (ou na configuração compartilhada da organização), por exemplo:"
                        )
                    } else {
                        format!(
                            "The response reached the output limit. `provider.max_output_tokens` is **{current}**; raise it in the repository `.cururu.toml` (or the organization's shared configuration), for example:"
                        )
                    }
                );
            }
            let _ = writeln!(
                body,
                "{}",
                if pt {
                    format!(
                        "Outra opção é reduzir `review.chunk_bytes` (hoje {chunk}) para que cada chamada revise um trecho menor. `LLM_MAX_OUTPUT_TOKENS` no ambiente tem precedência sobre o arquivo. Só os tokens gerados são cobrados: um limite maior aumenta o custo apenas no pior caso."
                    )
                } else {
                    format!(
                        "Alternatively, lower `review.chunk_bytes` (now {chunk}) so each call reviews a smaller part. `LLM_MAX_OUTPUT_TOKENS` in the environment takes precedence over the file. Only generated tokens are billed, so a higher limit raises the worst-case cost only."
                    )
                }
            );
        }
        "content_filter" => {
            let _ = writeln!(
                body,
                "{}",
                if pt {
                    "O provedor filtrou a resposta. Verifique as políticas de conteúdo do provedor ou escolha outro modelo em `provider.model`."
                } else {
                    "The provider filtered the response. Check the provider's content policies or choose another model in `provider.model`."
                }
            );
        }
        _ => {
            let _ = writeln!(
                body,
                "{}",
                if pt {
                    "O modelo devolveu um review inválido. Tente novamente; se continuar, escolha outro modelo em `provider.model` ou aumente `provider.max_output_tokens`."
                } else {
                    "The model returned an invalid review. Retry; if it keeps happening, choose another model in `provider.model` or raise `provider.max_output_tokens`."
                }
            );
        }
    }
    Diagnostic {
        title: if pt {
            "Review não concluído"
        } else {
            "Review could not complete"
        },
        body,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(language: &str, tokens: u32) -> AppConfig {
        let mut config = AppConfig::from_local_env().unwrap();
        config.review.language = language.into();
        config.llm.max_output_tokens = tokens;
        config.review.chunk_bytes = 45_000;
        config
    }

    #[test]
    fn truncation_names_the_key_current_value_and_suggestion() {
        let english = incomplete_review(&config("en-US", 8000), "length");
        assert_eq!(english.title, "Review could not complete");
        assert!(
            english
                .body
                .contains("`provider.max_output_tokens` is **8000**")
        );
        assert!(english.body.contains("max_output_tokens = 16000"));
        assert!(english.body.contains("`review.chunk_bytes` (now 45000)"));
        let portuguese = incomplete_review(&config("pt-BR", 4000), "max_tokens");
        assert_eq!(portuguese.title, "Review não concluído");
        assert!(portuguese.body.contains("está em **4000**"));
        assert!(portuguese.body.contains("max_output_tokens = 8000"));
    }

    #[test]
    fn suggestion_is_capped_and_other_reasons_get_their_own_advice() {
        let capped = incomplete_review(&config("en", 20_000), "length");
        assert!(capped.body.contains(&format!(
            "max_output_tokens = {MAX_SUGGESTED_OUTPUT_TOKENS}"
        )));
        let at_cap = incomplete_review(&config("en", MAX_SUGGESTED_OUTPUT_TOKENS), "length");
        assert!(!at_cap.body.contains("```toml"));
        assert!(at_cap.body.contains("review.chunk_bytes"));
        assert!(
            incomplete_review(&config("en", 8000), "content_filter")
                .body
                .contains("filtered")
        );
        assert!(
            incomplete_review(&config("en", 8000), "stop")
                .body
                .contains("invalid review")
        );
    }
}
