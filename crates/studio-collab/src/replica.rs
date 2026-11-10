//! Convergence: our changes apply at once and wait for their place in
//! the host's order; when someone else's change comes first, ours are
//! taken back, theirs applied, and ours applied again on top.

use studio_core::model::Id;
use studio_core::ops::Transaction;
use studio_core::Document;

#[derive(Default, Debug)]
pub struct Replica {
    /// The last sequence number applied.
    pub seq: u64,
    /// Our changes not yet ordered by the host: (forward, inverse).
    pending: Vec<(Transaction, Transaction)>,
}

impl Replica {
    pub fn new(seq: u64) -> Replica {
        Replica { seq, pending: Vec::new() }
    }

    /// Commits a local change and remembers it until acknowledged. Returns
    /// the transaction to send.
    pub fn local(&mut self, doc: &mut Document, tx: Transaction) -> Result<Transaction, studio_core::ops::OpError> {
        let (fwd, inv) = doc.commit_inv(tx)?;
        self.pending.push((fwd.clone(), inv));
        Ok(fwd)
    }

    /// Records an already applied change (undo/redo results).
    pub fn local_applied(&mut self, fwd: Transaction, inverse: Transaction) {
        self.pending.push((fwd, inverse));
    }

    pub fn ack(&mut self, seq: u64, tx: Id) {
        self.seq = self.seq.max(seq);
        self.pending.retain(|(f, _)| f.id != tx);
    }

    /// Applies someone else's change in order. Returns false when it
    /// could not apply (it conflicted; everyone skips it alike).
    pub fn remote(&mut self, doc: &mut Document, seq: u64, tx: &Transaction) -> bool {
        self.seq = self.seq.max(seq);
        if self.pending.is_empty() {
            return doc.apply_remote(tx).is_ok();
        }
        for (_, inv) in self.pending.iter().rev() {
            let _ = inv.apply(&mut doc.project);
        }
        let ok = doc.apply_remote(tx).is_ok();
        let mut kept = Vec::with_capacity(self.pending.len());
        for (fwd, _) in self.pending.drain(..) {
            if let Ok(inv) = fwd.apply(&mut doc.project) {
                kept.push((fwd, inv));
            }
        }
        self.pending = kept;
        doc.revision += 1;
        ok
    }

    pub fn pending(&self) -> usize {
        self.pending.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_core::model::*;
    use studio_core::ops::{Op, TrackField};

    #[test]
    fn concurrent_edits_converge() {
        let mut base = Project::new("p");
        let t = Track::new("Voix", TrackKind::Vocal);
        let id = t.id;
        base.tracks.push(t);
        // A and B both start from base; the host orders B's change first
        let mut a = Document::new(base.clone());
        let mut ra = Replica::new(0);
        let mut b = Document::new(base.clone());
        let mut rb = Replica::new(0);
        let ta = ra.local(&mut a, Transaction::new("a", Actor::User, vec![Op::SetTrack { id, field: TrackField::VolumeDb(-1.0) }, Op::SetTrack { id, field: TrackField::Pan(0.5) }])).unwrap();
        let tb = rb.local(&mut b, Transaction::new("b", Actor::User, vec![Op::SetTrack { id, field: TrackField::VolumeDb(-9.0) }])).unwrap();
        // order: tb (seq 1), ta (seq 2)
        rb.ack(1, tb.id);
        ra.remote(&mut a, 1, &tb);
        ra.ack(2, ta.id);
        rb.remote(&mut b, 2, &ta);
        assert_eq!(a.project, b.project);
        assert_eq!(a.project.tracks[0].volume_db, -1.0);
        assert_eq!(a.project.tracks[0].pan, 0.5);
        assert_eq!(ra.pending() + rb.pending(), 0);
    }
}
