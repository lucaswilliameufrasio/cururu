use std::future::Future;
use std::time::Duration;
use tokio::time::sleep;

pub async fn retry_with_backoff<F, Fut, T>(operation: F, max_retries: u32) -> anyhow::Result<T>
where
    F: Fn() -> Fut,
    Fut: Future<Output = anyhow::Result<T>>,
{
    let mut attempt = 0u32;
    loop {
        match operation().await {
            Ok(value) => return Ok(value),
            Err(e) => {
                if attempt >= max_retries {
                    return Err(e);
                }
                attempt += 1;
                let delay_ms = 200u64 * 2u64.pow(attempt - 1);
                sleep(Duration::from_millis(delay_ms)).await;
            }
        }
    }
}
