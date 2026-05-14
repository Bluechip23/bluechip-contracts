//! Native bank denom validator. Mirrors the cosmos-sdk `IsValidDenom`
//! regex `^[a-zA-Z][a-zA-Z0-9/:._-]{2,127}$` without pulling the
//! `regex` crate (which would balloon the wasm output).

use crate::error::{ContractError, InvalidDenomReason};

/// Validate a Cosmos SDK native bank denom against the documented
/// format rules: 3–128 characters, must start with an ASCII letter, and
/// the rest must be alphanumeric or one of `/`, `:`, `.`, `_`, `-`.
///
/// Catches the operator-typo class of failures at propose / instantiate
/// time rather than 48 hours later when an apply lands a malformed
/// denom and every subsequent `RequestExpansion` reverts inside the
/// bank module with an error nobody is watching for. Examples this
/// catches that the previous "non-empty after trim" check missed:
/// - `"Bluechip"`           (capital first letter — bank rejects)
/// - `"u bluechip"`         (whitespace inside)
/// - `"u"` or `"ub"`        (length < 3)
/// - `"1ubluechip"`         (digit prefix)
/// - `"ubluechip!"`         (punctuation outside the allowed set)
///
/// Accepts all the cosmos-sdk shapes this contract actually wants:
/// - `"ubluechip"`          (canonical native denom)
/// - `"ucustom"`            (test fixture)
/// - `"ibc/27394FB..."`     (IBC-wrapped — slashes + hex)
/// - `"factory/cosmos1.../tokenname"` (tokenfactory shape)
pub fn validate_native_denom(denom: &str) -> Result<(), ContractError> {
    let len = denom.len();
    if !(3..=128).contains(&len) {
        return Err(ContractError::InvalidDenom {
            denom: denom.to_string(),
            reason: InvalidDenomReason::LengthOutOfRange { len },
        });
    }
    let mut chars = denom.chars();
    let Some(first) = chars.next() else {
        // Length is in [3, 128] above, so an empty `chars()` would be a
        // logic bug — surface explicitly rather than panicking.
        return Err(ContractError::InvalidDenom {
            denom: denom.to_string(),
            reason: InvalidDenomReason::LengthOutOfRange { len: 0 },
        });
    };
    if !first.is_ascii_alphabetic() {
        return Err(ContractError::InvalidDenom {
            denom: denom.to_string(),
            reason: InvalidDenomReason::BadLeadingCharacter { first },
        });
    }
    for ch in chars {
        let allowed = ch.is_ascii_alphanumeric() || matches!(ch, '/' | ':' | '.' | '_' | '-');
        if !allowed {
            return Err(ContractError::InvalidDenom {
                denom: denom.to_string(),
                reason: InvalidDenomReason::DisallowedCharacter { ch },
            });
        }
    }
    Ok(())
}
