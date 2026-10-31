//! A submission's completion, which runs exactly once on whichever thread ends
//! the work. Losing a ticket cannot lose the completion: dropping it, even
//! while a thread unwinds, reports that the document stopped.

pub(crate) const STOPPED: &str = "document thread stopped";

type Complete<V> = Box<dyn FnOnce(Result<V, String>) + Send>;

pub struct Ticket<V> {
    complete: Option<Complete<V>>,
}

impl<V> Ticket<V> {
    pub fn new(complete: impl FnOnce(Result<V, String>) + Send + 'static) -> Self {
        Self {
            complete: Some(Box::new(complete)),
        }
    }

    pub(crate) fn finish(mut self, result: Result<V, String>) {
        if let Some(complete) = self.complete.take() {
            complete(result);
        }
    }
}

impl<V> Drop for Ticket<V> {
    fn drop(&mut self) {
        if let Some(complete) = self.complete.take() {
            complete(Err(STOPPED.to_string()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn counted() -> (Ticket<u8>, mpsc::Receiver<Result<u8, String>>) {
        let (sender, results) = mpsc::channel();
        let ticket = Ticket::new(move |result| sender.send(result).unwrap());
        (ticket, results)
    }

    #[test]
    fn finishing_completes_once_and_dropping_reports_a_stopped_document() {
        let (ticket, results) = counted();
        ticket.finish(Ok(7));
        assert_eq!(results.try_iter().collect::<Vec<_>>(), [Ok(7)]);

        let (ticket, results) = counted();
        drop(ticket);
        assert_eq!(
            results.try_iter().collect::<Vec<_>>(),
            [Err(STOPPED.to_string())]
        );
    }

    #[test]
    fn a_thread_that_unwinds_still_completes_the_tickets_it_held() {
        let (ticket, results) = counted();
        let unwound = std::thread::spawn(move || {
            let _held = ticket;
            panic!("the owner dies");
        })
        .join();
        assert!(unwound.is_err());
        assert_eq!(
            results.try_iter().collect::<Vec<_>>(),
            [Err(STOPPED.to_string())]
        );
    }
}
