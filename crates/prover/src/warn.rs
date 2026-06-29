//! Privacy warnings surfaced at the tooling layer (OPAQ.md A.8 / A.12). Opaq's
//! stance is education here, not restriction in the protocol — the contract
//! never blocks these, so the CLI must.

/// A.12: amounts are public, so the anonymity set is only identical (token,
/// amount) transfers. Flag non-round amounts as especially self-identifying.
pub fn amount(amount: u64) {
    eprintln!("\n! privacy (A.12): amount {amount} is PUBLIC on-chain — your anonymity set is");
    eprintln!("  only deposits/withdrawals of the SAME token AND the SAME amount.");
    if !is_round(amount) {
        eprintln!("  {amount} is not a round denomination, so that set is likely tiny and");
        eprintln!("  self-identifying. Prefer a common round amount (Phase 2 hides amounts).");
    }
}

/// A.8: a recipient with prior on-chain history re-links the withdrawal.
/// `history` is the recipient's prior-signature count when checked over RPC
/// (`Some`), or `None` when no RPC endpoint was given.
pub fn recipient(recipient: &[u8; 32], history: Option<RecipientHistory>) {
    let b58 = bs58::encode(recipient).into_string();
    eprintln!("\n! privacy (A.8): withdrawing to {}.", short(&b58));
    eprintln!("  If this address has any prior on-chain history — or was funded from a linked");
    eprintln!("  wallet or a CEX — it re-links the withdrawal to you. Prefer a FRESH address,");
    eprintln!("  and fund its first transaction carefully (not from a linked source).");
    match history {
        None => eprintln!(
            "  (Pass --rpc <url> to auto-check this address's on-chain history.)"
        ),
        Some(RecipientHistory { count: 0, .. }) => eprintln!(
            "  RPC check: no prior signatures seen — this address looks FRESH. (Still make \
             sure it wasn't funded from a linked source.)"
        ),
        Some(RecipientHistory { count, capped }) => {
            let n = if capped { format!("≥{count}") } else { count.to_string() };
            eprintln!(
                "  ⚠ RPC check: this address has {n} prior signature(s) — it is NOT fresh. \
                 Using it re-links this withdrawal. Withdraw to a brand-new address instead."
            );
        }
    }
}

/// Result of a recipient on-chain history lookup (A.8): how many prior
/// signatures the address has, and whether that count hit the RPC page cap.
pub struct RecipientHistory {
    pub count: usize,
    pub capped: bool,
}

fn is_round(n: u64) -> bool {
    matches!(n, 1 | 5 | 10 | 25 | 50 | 100 | 500 | 1000) || (n != 0 && n % 1000 == 0)
}

fn short(s: &str) -> String {
    if s.len() > 12 {
        format!("{}…{}", &s[..6], &s[s.len() - 4..])
    } else {
        s.to_string()
    }
}
