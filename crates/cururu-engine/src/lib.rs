use anyhow::Result;
use async_trait::async_trait;
use cururu_core::{DiffChunk, ReviewResult};

#[derive(Debug, Clone)]
pub struct ReviewUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
    pub cached_tokens: u32,
    pub reasoning_tokens: u32,
    pub cost: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct ChunkResult {
    pub review: ReviewResult,
    pub usage: Option<ReviewUsage>,
}

/// Produces normalized Cururu review results for one bounded diff chunk.
#[async_trait]
pub trait ReviewAgent: Send + Sync {
    async fn review_chunk(&self, chunk: &DiffChunk) -> Result<ChunkResult>;

    async fn answer_question(
        &self,
        _tone: &str,
        _technical_level: &str,
        _question: &str,
        _context: &str,
    ) -> Result<String> {
        anyhow::bail!("LLM adapter does not support conversation answers")
    }
}

/// Reviews chunks sequentially and returns no partial result after a failure.
pub async fn review_chunks(
    agent: &dyn ReviewAgent,
    chunks: &[DiffChunk],
) -> Result<Vec<ChunkResult>> {
    let mut results = Vec::with_capacity(chunks.len());
    for chunk in chunks {
        results.push(agent.review_chunk(chunk).await?);
    }
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::{ChunkResult, ReviewAgent, ReviewUsage, review_chunks};
    use anyhow::Result;
    use async_trait::async_trait;
    use cururu_core::{DiffChunk, ReviewResult};
    use std::sync::{Arc, Mutex};

    struct RecordingAgent {
        seen: Arc<Mutex<Vec<usize>>>,
        fail_on: Option<usize>,
    }

    #[async_trait]
    impl ReviewAgent for RecordingAgent {
        async fn review_chunk(&self, chunk: &DiffChunk) -> Result<ChunkResult> {
            self.seen.lock().unwrap().push(chunk.index);
            if self.fail_on == Some(chunk.index) {
                anyhow::bail!("review failed");
            }
            Ok(ChunkResult {
                review: ReviewResult {
                    model: "test-model".into(),
                    files_reviewed: chunk.files.len(),
                    summary: String::new(),
                    findings: Vec::new(),
                },
                usage: Some(ReviewUsage {
                    prompt_tokens: 1,
                    completion_tokens: 2,
                    total_tokens: 3,
                    cached_tokens: 0,
                    reasoning_tokens: 0,
                    cost: None,
                }),
            })
        }
    }

    fn chunk(index: usize) -> DiffChunk {
        DiffChunk {
            index,
            text: format!("diff {index}"),
            files: vec![format!("src/{index}.rs")],
        }
    }

    #[tokio::test]
    async fn reviews_chunks_sequentially_and_preserves_order() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let agent = RecordingAgent {
            seen: seen.clone(),
            fail_on: None,
        };

        let results = review_chunks(&agent, &[chunk(0), chunk(1)]).await.unwrap();

        assert_eq!(*seen.lock().unwrap(), [0, 1]);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].review.model, "test-model");
        assert_eq!(results[1].review.files_reviewed, 1);
    }

    #[tokio::test]
    async fn stops_at_first_failed_chunk_without_returning_partial_results() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let agent = RecordingAgent {
            seen: seen.clone(),
            fail_on: Some(1),
        };

        let result = review_chunks(&agent, &[chunk(0), chunk(1), chunk(2)]).await;

        assert!(result.is_err());
        assert_eq!(*seen.lock().unwrap(), [0, 1]);
    }
}
