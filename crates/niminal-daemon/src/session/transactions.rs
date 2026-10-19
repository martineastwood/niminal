//! Graph transaction receipts acknowledge the sample at which DSP actually swaps.
use super::*;
use serde_json::{Value as Json, json};
pub(super) const WAITING: u64 = u64::MAX;
const CANCELLED: u64 = u64::MAX - 1;
const DROPPED: u64 = u64::MAX - 2;

pub(super) struct Transaction {
    pub id: u64,
    pub client: ClientId,
    pub at: u64,
    pub applied: Arc<AtomicU64>,
    announced: Option<u64>,
}
impl Transaction {
    fn json(&self) -> Json {
        let applied = self.applied.load(Ordering::Acquire);
        json!({ "id": self.id, "at": self.at, "status": match applied {
            WAITING => "queued", CANCELLED => "cancelled", DROPPED => "dropped", _ => "applied"
        }, "applied_at": (applied < DROPPED).then_some(applied) })
    }
}
impl Session {
    pub(super) fn accept_transactions(
        &mut self,
        accepted: &[(crate::events::TransactionReceipt, Arc<AtomicU64>)],
        client: ClientId,
    ) {
        for (r, applied) in accepted {
            self.transactions.push_back(Transaction {
                id: r.id,
                at: r.at,
                client,
                applied: applied.clone(),
                announced: None,
            });
        }
        self.reconcile_transactions();
        // Pending receipts are retained; terminal history is bounded.
        let mut finished = self
            .transactions
            .iter()
            .filter(|t| t.applied.load(Ordering::Acquire) != WAITING)
            .count();
        self.transactions.retain(|t| {
            if finished > 256 && t.applied.load(Ordering::Acquire) != WAITING {
                finished -= 1;
                false
            } else {
                true
            }
        });
    }
    pub(super) fn reconcile_transactions(&self) {
        for t in &self.transactions {
            if t.applied.load(Ordering::Acquire) == WAITING
                && !self.pending.iter().any(|p| p.id == t.id)
                && self
                    .current_transaction
                    .as_ref()
                    .is_none_or(|a| !Arc::ptr_eq(a, &t.applied))
            {
                let _ = t.applied.compare_exchange(
                    WAITING,
                    DROPPED,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                );
            }
        }
    }
    pub(super) fn cancel_transaction(&self, id: u64) {
        if let Some(t) = self.transactions.iter().find(|t| t.id == id) {
            t.applied.store(CANCELLED, Ordering::Release);
        }
    }
    pub fn transaction_status(&self, client: ClientId) -> Json {
        json!(
            self.transactions
                .iter()
                .filter(|t| t.client == client)
                .map(Transaction::json)
                .collect::<Vec<_>>()
        )
    }
    pub fn take_transaction_updates(&mut self) -> Vec<(ClientId, Json)> {
        self.transactions
            .iter_mut()
            .filter_map(|t| {
                let state = t.applied.load(Ordering::Acquire);
                if t.announced == Some(state) {
                    None
                } else {
                    t.announced = Some(state);
                    Some((t.client, t.json()))
                }
            })
            .collect()
    }
}
