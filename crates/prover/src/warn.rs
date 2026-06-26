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
pub fn recipient(recipient: &[u8; 32]) {
    let b58 = bs58::encode(recipient).into_string();
    eprintln!("\n! privacy (A.8): withdrawing to {}.", short(&b58));
    eprintln!("  If this address has any prior on-chain history — or was funded from a linked");
    eprintln!("  wallet or a CEX — it re-links the withdrawal to you. Prefer a FRESH address,");
    eprintln!("  and fund its first transaction carefully (not from a linked source).");
    eprintln!("  (Auto-checking the address's history over RPC is the remaining polish.)");
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
