//! A submission's completion, which runs exactly once on whichever thread ends
//! the work. Losing a ticket cannot lose the completion: dropping it, even
//! while a thread unwinds, reports that the document stopped.

use crate::DocumentError;

type Complete<V> = Box<dyn FnOnce(Result<V, DocumentError>) + Send>;

pub struct Ticket<V> {
    complete: Option<Complete<V>>,
}

impl<V> Ticket<V> {
    pub fn new(complete: impl FnOnce(Result<V, DocumentError>) + Send + 'static) -> Self {
        Self {
            complete: Some(Box::new(complete)),
        }
    }

    pub(crate) fn finish(mut self, result: Result<V, DocumentError>) {
        if let Some(complete) = self.complete.take() {
            complete(result);
        }
    }
}

impl<V> Drop for Ticket<V> {
    fn drop(&mut self) {
        if let Some(complete) = self.complete.take() {
            complete(Err(DocumentError::Stopped));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn counted() -> (Ticket<u8>, mpsc::Receiver<Result<u8, DocumentError>>) {
        let (sender, results) = mpsc::channel();
        let ticket = Ticket::new(move |result| sender.send(result).unwrap());
        (ticket, results)
    }

    fn stopped(results: &mpsc::Receiver<Result<u8, DocumentError>>) -> bool {
        let all = results.try_iter().collect::<Vec<_>>();
        matches!(all.as_slice(), [Err(DocumentError::Stopped)])
    }

    #[test]
    fn finishing_completes_once_and_dropping_reports_a_stopped_document() {
        let (ticket, results) = counted();
        ticket.finish(Ok(7));
        let all = results.try_iter().collect::<Vec<_>>();
        assert!(matches!(all.as_slice(), [Ok(7)]), "{all:?}");

        let (ticket, results) = counted();
        drop(ticket);
        assert!(stopped(&results));
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
        assert!(stopped(&results));
    }
}
