//! Dispatch authentication, bound to the plan, worker and Server epoch.
use crate::contracts::{canonical_bytes, string, u64_field};
use crate::{ControlError, Result};
use hmac::{Hmac, Mac};
use serde_json::{Value, json};
use sha2::Sha256;

fn message(
    lease: &Value,
    plan: &Value,
    worker_id: &str,
    remaining: u64,
    usage: &Value,
) -> Result<Vec<u8>> {
    canonical_bytes(&json!([
        lease["lease_id"],
        lease["epoch"],
        lease["expires_at_ms"],
        plan["plan_digest"],
        worker_id,
        remaining,
        usage
    ]))
}
#[allow(clippy::too_many_arguments)]
pub fn issue(
    key: &[u8],
    worker_id: &str,
    plan: &Value,
    epoch: u64,
    expires_at_ms: u64,
    remaining: u64,
    usage: &Value,
) -> Result<Value> {
    if key.len() < 32 {
        return Err(ControlError::new("LEASE_KEY_TOO_SHORT"));
    }
    let mut nonce = [0_u8; 32];
    getrandom::fill(&mut nonce).map_err(|_| ControlError::new("ENTROPY_UNAVAILABLE"))?;
    let id = nonce.iter().map(|b| format!("{b:02x}")).collect::<String>();
    let mut lease = json!({"lease_id":id,"epoch":epoch,"expires_at_ms":expires_at_ms});
    let mut mac =
        Hmac::<Sha256>::new_from_slice(key).map_err(|_| ControlError::new("INVALID_LEASE_KEY"))?;
    mac.update(&message(&lease, plan, worker_id, remaining, usage)?);
    lease["token"] = json!(
        mac.finalize()
            .into_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    );
    Ok(lease)
}
pub fn verify(
    key: &[u8],
    worker_id: &str,
    dispatch: &Value,
    epoch: u64,
    now_ms: u64,
) -> Result<()> {
    let lease = &dispatch["lease"];
    let plan = &dispatch["plan"];
    if key.len() < 32
        || u64_field(lease, "epoch")? != epoch
        || u64_field(lease, "expires_at_ms")? <= now_ms
    {
        return Err(ControlError::new("STALE_LEASE"));
    }
    let token = string(lease, "token")?;
    if token.len() != 64 || !token.is_ascii() {
        return Err(ControlError::new("INVALID_LEASE_TOKEN"));
    }
    let bytes = (0..64)
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&token[i..i + 2], 16)
                .map_err(|_| ControlError::new("INVALID_LEASE_TOKEN"))
        })
        .collect::<Result<Vec<_>>>()?;
    let mut mac =
        Hmac::<Sha256>::new_from_slice(key).map_err(|_| ControlError::new("INVALID_LEASE_KEY"))?;
    let remaining = u64_field(dispatch, "remaining_timeout_ms")?;
    mac.update(&message(
        lease,
        plan,
        worker_id,
        remaining,
        &dispatch["consumed_usage"],
    )?);
    mac.verify_slice(&bytes)
        .map_err(|_| ControlError::new("INVALID_LEASE_TOKEN"))?;
    // The grant is authenticated. Recomputing it from the Worker's wall clock
    // would reject valid packets after transit and introduce clock-skew drift.
    if remaining == 0 || remaining > u64_field(&plan["limits"], "total_timeout_ms")? {
        return Err(ControlError::new("DISPATCH_BUDGET_INCREASED"));
    }
    Ok(())
}
