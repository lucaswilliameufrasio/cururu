use std::{future::Future, time::Duration};

pub async fn retry_with_backoff<F, Fut, T>(operation: F, max_retries: u32) -> anyhow::Result<T>
where
    F: Fn() -> Fut,
    Fut: Future<Output = anyhow::Result<T>>,
{
    let mut attempt = 0_u32;
    loop {
        match operation().await {
            Ok(value) => return Ok(value),
            Err(error) => {
                if attempt >= max_retries {
                    return Err(error);
                }
                attempt += 1;
                let delay_ms = 200_u64 * 2_u64.pow(attempt - 1);
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }
        }
    }
}
