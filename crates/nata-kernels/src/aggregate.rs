//! Exact aggregate state shared by accumulation, merge and spill replay.
//!
//! ADR 0010's bound: fewer than u64::MAX non-NULL addends, each of magnitude
//! below 10^38, need fewer than 192 signed bits. AVG can widen by at most six
//! decimal places (fewer than 212 bits). i256 therefore cannot overflow on a
//! valid state, regardless of partial/merge order. Checks remain on every edge,
//! including replay: a spill file is not trusted to satisfy these bounds.
use arrow_buffer::i256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AggregateError {
    Overflow,
    InvalidState,
    InvalidType,
}

/// One exact SUM/AVG partial. Empty and all-NULL have count zero and sum zero.
/// COUNT(*) is deliberately separate: its count includes NULL rows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExactSum {
    sum: i256,
    count: u64,
}

impl ExactSum {
    /// Update one *selected*, valid argument. NULL must be skipped by the caller.
    /// Does not narrow the partial to the final SQL result's precision.
    pub fn add(&mut self, value: i128) -> Result<(), AggregateError> {
        if !fits(value, 38) {
            return Err(AggregateError::InvalidState);
        }
        let count = checked_count(self.count, 1)?;
        let sum = self
            .sum
            .checked_add(i256::from_i128(value))
            .ok_or(AggregateError::Overflow)?;
        self.sum = sum;
        self.count = count;
        Ok(())
    }

    /// Combine a resident or validated spilled partial exactly once. No final
    /// SUM precision check here: later partials may cancel a large intermediate.
    pub fn merge(&mut self, other: Self) -> Result<(), AggregateError> {
        let count = checked_count(self.count, other.count)?;
        let sum = self
            .sum
            .checked_add(other.sum)
            .ok_or(AggregateError::Overflow)?;
        self.sum = sum;
        self.count = count;
        Ok(())
    }

    pub fn count(&self) -> u64 {
        self.count
    }

    /// Fixed little-endian wide state, never native Rust layout. IPC's trusted
    /// schema stores these bytes, not an externally visible SQL Decimal256.
    pub fn spill_sum(&self) -> [u8; 32] {
        self.sum.to_le_bytes()
    }

    /// Validate wide replay before merging. Even an IPC payload with a repaired
    /// checksum must not smuggle an impossible sum/count past the count bound.
    pub fn from_spill(bytes: [u8; 32], count: u64) -> Result<Self, AggregateError> {
        if count == u64::MAX {
            return Err(AggregateError::InvalidState);
        }
        let sum = i256::from_le_bytes(bytes);
        let bound = i256::from_i128(decimal_limit(38)?)
            .checked_sub(i256::ONE)
            .and_then(|v| v.checked_mul(i256::from_i128(i128::from(count))))
            .ok_or(AggregateError::InvalidState)?;
        if sum.checked_abs().is_none_or(|v| v > bound) {
            return Err(AggregateError::InvalidState);
        }
        Ok(Self { sum, count })
    }

    /// SUM(DECIMAL) and SUM(BIGINT). Narrow exactly once, at group finalization,
    /// before HAVING. Empty/all-NULL returns NULL.
    pub fn decimal_sum(&self, precision: u8) -> Result<Option<i128>, AggregateError> {
        if precision == 0 {
            return Err(AggregateError::InvalidType);
        }
        decimal_limit(precision)?;
        if self.count == 0 {
            return Ok(None);
        }
        let value = self.sum.to_i128().ok_or(AggregateError::Overflow)?;
        if !fits(value, precision) {
            return Err(AggregateError::Overflow);
        }
        Ok(Some(value))
    }

    /// SUM(TINYINT/SMALLINT/INTEGER) -> BIGINT, with a final range check.
    pub fn integer_sum(&self) -> Result<Option<i64>, AggregateError> {
        self.decimal_sum(38)?
            .map(|n| i64::try_from(n).map_err(|_| AggregateError::Overflow))
            .transpose()
    }

    /// Exact AVG under decimal-rules section 8, half away from zero. Do not
    /// compute/narrow SUM first: an overflowing SUM may have a representable AVG.
    pub fn average(&self, precision: u8, scale: u8) -> Result<Option<i128>, AggregateError> {
        let (out_precision, out_scale) = average_type(precision, scale)?;
        if self.count == 0 {
            return Ok(None);
        }
        let increase = out_scale
            .checked_sub(scale)
            .ok_or(AggregateError::InvalidType)?;
        let factor = i256::from_i128(decimal_limit(increase)?);
        let numerator = self
            .sum
            .checked_mul(factor)
            .ok_or(AggregateError::Overflow)?;
        let denominator = i256::from_i128(i128::from(self.count));
        let (q, remainder) = numerator
            .checked_div_rem(denominator)
            .ok_or(AggregateError::Overflow)?;
        let twice = remainder
            .checked_abs()
            .and_then(|v| v.checked_mul(i256::from_i128(2)))
            .ok_or(AggregateError::Overflow)?;
        let rounded = if twice >= denominator {
            q.checked_add(if numerator < i256::ZERO {
                i256::MINUS_ONE
            } else {
                i256::ONE
            })
            .ok_or(AggregateError::Overflow)?
        } else {
            q
        };
        let value = rounded.to_i128().ok_or(AggregateError::Overflow)?;
        if !fits(value, out_precision) {
            return Err(AggregateError::Overflow);
        }
        Ok(Some(value))
    }
}

/// Type of AVG(DECIMAL(p,s)); integers supply precisions 3/5/10/19, scale 0.
pub fn average_type(precision: u8, scale: u8) -> Result<(u8, u8), AggregateError> {
    if !(1..=38).contains(&precision) || scale > precision {
        return Err(AggregateError::InvalidType);
    }
    let integer = precision
        .checked_sub(scale)
        .ok_or(AggregateError::InvalidType)?;
    let fractional = scale
        .checked_add(6)
        .ok_or(AggregateError::InvalidType)?
        .min(
            38u8.checked_sub(integer)
                .ok_or(AggregateError::InvalidType)?,
        );
    Ok((
        integer
            .checked_add(fractional)
            .ok_or(AggregateError::InvalidType)?,
        fractional,
    ))
}

/// Count guards apply to updates *and* merges, even if only SUM is requested.
/// Reserve u64::MAX as an invalid state (ADR 0010's strict count bound).
pub fn checked_count(left: u64, right: u64) -> Result<u64, AggregateError> {
    left.checked_add(right)
        .filter(|&n| n < u64::MAX)
        .ok_or(AggregateError::Overflow)
}

/// SQL COUNT result is non-NULL BIGINT, with its own narrower final bound.
pub fn finish_count(count: u64) -> Result<i64, AggregateError> {
    i64::try_from(count).map_err(|_| AggregateError::Overflow)
}

// Zero is accepted here as 10^0, for AVG's unchanged-scale cases.
fn decimal_limit(precision: u8) -> Result<i128, AggregateError> {
    if precision > 38 {
        return Err(AggregateError::InvalidType);
    }
    10i128
        .checked_pow(u32::from(precision))
        .ok_or(AggregateError::InvalidType)
}
fn fits(value: i128, precision: u8) -> bool {
    decimal_limit(precision).is_ok_and(|limit| value.unsigned_abs() < limit.unsigned_abs())
}
