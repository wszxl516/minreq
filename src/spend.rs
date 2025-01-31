use std::{future::Future, pin::Pin, time::Duration, task::Poll, task::Context, time::Instant};
pub struct SpendFuture<Fut: Future + Unpin> {
    fut: Fut,
    started_at: Option<Instant>,
}

impl<Fut: Future + Unpin> SpendFuture<Fut> {
    pub fn new(fut: Fut, started_at: Option<Instant>) -> Self {
        Self { fut, started_at }
    }
}

impl<Fut: Future + Unpin> Future for SpendFuture<Fut> {
    type Output = (Fut::Output, Duration);

    fn poll(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Self::Output> {
        let Self { fut, started_at } = self.get_mut();
        let started_at = started_at.get_or_insert_with(Instant::now);
        match Pin::new(fut).poll(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(out) => {
                let elapsed = started_at.elapsed();
                Poll::Ready((out, elapsed))
            }
        }
    }
}
