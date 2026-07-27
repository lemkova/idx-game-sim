//! IDX (Indonesia Stock Exchange) market microstructure rules.
//!
//! - Tick size fractions (regular market): <200 -> 1, 200-<500 -> 2,
//!   500-<2000 -> 5, 2000-<5000 -> 10, >=5000 -> 25.
//! - Symmetric auto rejection (ARA/ARB): prev close 50-200 -> 35%,
//!   >200-5000 -> 25%, >5000 -> 20%. Regular-market floor is Rp 50.
//! - Typical retail fees: 0.15% buy, 0.25% sell (incl. 0.1% final sales tax).

use crate::types::Price;

pub fn tick_size(price: Price) -> Price {
    if price < 200 {
        1
    } else if price < 500 {
        2
    } else if price < 2000 {
        5
    } else if price < 5000 {
        10
    } else {
        25
    }
}

pub fn valid_price(price: Price) -> bool {
    price >= 50 && price % tick_size(price) == 0
}

/// Next valid price above `price` (assumes `price` is on the grid).
pub fn tick_up(price: Price) -> Price {
    price + tick_size(price)
}

/// Next valid price below `price` (assumes `price` is on the grid).
/// Handles band boundaries: tick_down(200) == 199, tick_down(5000) == 4990.
pub fn tick_down(price: Price) -> Price {
    (price - tick_size(price - 1)).max(1)
}

/// Snap down onto the tick grid (identity when already valid).
pub fn snap_down(price: Price) -> Price {
    price - price.rem_euclid(tick_size(price))
}

/// Snap up onto the tick grid (identity when already valid).
pub fn snap_up(price: Price) -> Price {
    let d = snap_down(price);
    if d == price {
        price
    } else {
        tick_up(d)
    }
}

pub fn auto_reject_pct(prev_close: Price) -> f64 {
    if prev_close <= 200 {
        0.35
    } else if prev_close <= 5000 {
        0.25
    } else {
        0.20
    }
}

/// (ARB lower bound, ARA upper bound) for a session, both on the tick grid.
pub fn auto_reject_bounds(prev_close: Price) -> (Price, Price) {
    let pct = auto_reject_pct(prev_close);
    let upper = snap_down((prev_close as f64 * (1.0 + pct)).floor() as Price);
    let lower_raw = (prev_close as f64 * (1.0 - pct)).ceil() as Price;
    let lower = snap_up(lower_raw.max(50));
    (lower, upper)
}

pub const FEE_BUY: f64 = 0.0015;
pub const FEE_SELL: f64 = 0.0025;

pub fn fee_buy(value: i64) -> i64 {
    (value as f64 * FEE_BUY).round() as i64
}

pub fn fee_sell(value: i64) -> i64 {
    (value as f64 * FEE_SELL).round() as i64
}

// ---- formatting helpers ----

pub fn thousands(n: i64) -> String {
    let neg = n < 0;
    let mut v = n.unsigned_abs();
    let mut parts: Vec<String> = Vec::new();
    loop {
        if v < 1000 {
            parts.push(v.to_string());
            break;
        }
        parts.push(format!("{:03}", v % 1000));
        v /= 1000;
    }
    parts.reverse();
    let body = parts.join(",");
    if neg {
        format!("-{body}")
    } else {
        body
    }
}

pub fn signed_thousands(n: i64) -> String {
    if n > 0 {
        format!("+{}", thousands(n))
    } else {
        thousands(n)
    }
}

/// Compact volume/value: 1.2K, 34.5M, 1.9B.
pub fn compact(n: i64) -> String {
    let a = n.abs() as f64;
    let s = if a >= 1e9 {
        format!("{:.1}B", n as f64 / 1e9)
    } else if a >= 1e6 {
        format!("{:.1}M", n as f64 / 1e6)
    } else if a >= 10_000.0 {
        format!("{:.1}K", n as f64 / 1e3)
    } else {
        return thousands(n);
    };
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_bands() {
        assert_eq!(tick_size(199), 1);
        assert_eq!(tick_size(200), 2);
        assert_eq!(tick_size(499), 2);
        assert_eq!(tick_size(500), 5);
        assert_eq!(tick_size(1999), 5);
        assert_eq!(tick_size(2000), 10);
        assert_eq!(tick_size(4990), 10);
        assert_eq!(tick_size(5000), 25);
    }

    #[test]
    fn tick_walk_over_boundaries() {
        assert_eq!(tick_up(199), 200);
        assert_eq!(tick_down(200), 199);
        assert_eq!(tick_up(498), 500);
        assert_eq!(tick_down(500), 498);
        assert_eq!(tick_up(1995), 2000);
        assert_eq!(tick_down(2000), 1995);
        assert_eq!(tick_up(4990), 5000);
        assert_eq!(tick_down(5000), 4990);
        assert_eq!(tick_up(9750), 9775);
    }

    #[test]
    fn snapping() {
        assert_eq!(snap_down(201), 200);
        assert_eq!(snap_up(201), 202);
        assert_eq!(snap_down(5013), 5000);
        assert_eq!(snap_up(5013), 5025);
        assert_eq!(snap_down(1545), 1545);
        assert_eq!(snap_up(1545), 1545);
    }

    #[test]
    fn auto_reject() {
        // prev 157 -> 35% band
        let (lo, hi) = auto_reject_bounds(157);
        assert!(lo >= 50 && valid_price(lo));
        assert!(valid_price(hi));
        assert!(lo <= 103 && hi >= 211 - 10); // roughly +-35%
        // prev 9750 -> 20% band
        let (lo, hi) = auto_reject_bounds(9750);
        assert!(valid_price(lo) && valid_price(hi));
        assert!(lo >= 7800 - 25 && hi <= 11700);
    }

    #[test]
    fn formatting() {
        assert_eq!(thousands(1234567), "1,234,567");
        assert_eq!(thousands(-1500), "-1,500");
        assert_eq!(signed_thousands(42), "+42");
    }
}
