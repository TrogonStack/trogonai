#[cfg(test)]
pub static TRACER_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[cfg(test)]
pub async fn tracer_test_guard() -> tokio::sync::MutexGuard<'static, ()> {
    TRACER_TEST_LOCK.lock().await
}
