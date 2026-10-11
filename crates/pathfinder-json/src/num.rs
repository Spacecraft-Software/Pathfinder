//! Integer / decimal numbers.
use super::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cmp::Ordering;
use core::fmt;
use core::hash::{Hash, Hasher};
use num_bigint::{BigInt, BigUint, Sign};
use num_traits::cast::ToPrimitive;

/// Integer / decimal number.
///
/// The speciality of this type is that numbers are distinguished into
/// integers and 64-bit floating-point numbers.
/// This allows using integers to index arrays,
/// while using floating-point numbers to do general math.
///
/// Operations on numbers follow a few principles:
/// * The sum, difference, product, and remainder of two integers is integer.
/// * Any other operation between two numbers yields a float.
#[derive(Clone, Debug)]
pub enum Num {
    /// Machine-size integer
    Int(isize),
    /// Arbitrarily large integer
    BigInt(Rc<BigInt>),
    /// Floating-point number
    Float(f64),
    /// Decimal number
    Dec(Rc<String>),
}

impl Num {
    /// Create a big integer.
    pub fn big_int(i: BigInt) -> Self {
        Self::BigInt(i.into())
    }

    pub(crate) fn from_str(s: &str) -> Self {
        Self::from_str_radix(s, 10).unwrap_or_else(|| Self::Dec(Rc::new(s.to_string())))
    }

    /// Convert from an integral type to a machine-sized or big integer.
    pub fn from_integral<T: Copy + TryInto<isize> + Into<BigInt>>(x: T) -> Self {
        x.try_into()
            .map_or_else(|_| Num::big_int(x.into()), Num::Int)
    }

    /// Try to parse an integer from a string with given radix.
    pub fn from_str_radix(i: &str, radix: u32) -> Option<Self> {
        let int = || isize::from_str_radix(i, radix).ok().map(Num::Int);
        let big = || bigint_from_str_radix(i, radix).map(Self::big_int);
        int().or_else(big)
    }

    /// Try to parse a decimal string to a [`Self::Float`], else return NaN.
    pub fn from_dec_str(n: &str) -> Self {
        // TODO: changed to NaN!
        n.parse().map_or(Self::Float(f64::NAN), Self::Float)
    }

    pub(crate) fn is_int(&self) -> bool {
        matches!(self, Self::Int(_) | Self::BigInt(_))
    }

    /// If the value is a machine-sized integer, return it, else fail.
    pub fn as_isize(&self) -> Option<isize> {
        match self {
            Self::Int(i) => Some(*i),
            Self::BigInt(i) => i.to_isize(),
            _ => None,
        }
    }

    pub(crate) fn as_pos_usize(&self) -> Option<PosUsize> {
        match self {
            Self::Int(i) => Some(PosUsize(*i >= 0, i.unsigned_abs())),
            Self::BigInt(i) => i
                .magnitude()
                .to_usize()
                .map(|u| PosUsize(i.sign() != Sign::Minus, u)),
            _ => None,
        }
    }

    /// If the value is or can be converted to float, return it, else fail.
    pub(crate) fn as_f64(&self) -> f64 {
        match self {
            Self::Int(n) => *n as f64,
            Self::BigInt(n) => n.to_f64().unwrap(),
            Self::Float(n) => *n,
            Self::Dec(n) => n.parse().unwrap(),
        }
    }

    pub(crate) fn length(&self) -> Self {
        match self {
            Self::Int(i) => Self::Int(i.abs()),
            Self::BigInt(i) => match i.sign() {
                Sign::Plus | Sign::NoSign => Self::BigInt(i.clone()),
                Sign::Minus => Self::BigInt(BigInt::from(i.magnitude().clone()).into()),
            },
            Self::Dec(n) => Self::from_dec_str(n).length(),
            Self::Float(f) => Self::Float(f.abs()),
        }
    }
}

#[derive(Copy, Clone)]
pub(crate) struct PosUsize(pub(crate) bool, pub(crate) usize);

impl PosUsize {
    pub fn wrap(&self, len: usize) -> Option<usize> {
        self.0.then_some(self.1).or_else(|| len.checked_sub(self.1))
    }
}

#[test]
fn wrap_test() {
    let len = 4;
    let pos = |i| PosUsize(true, i);
    let neg = |i| PosUsize(false, i);
    assert_eq!(pos(0).wrap(len), Some(0));
    assert_eq!(pos(8).wrap(len), Some(8));
    assert_eq!(neg(1).wrap(len), Some(3));
    assert_eq!(neg(4).wrap(len), Some(0));
    assert_eq!(neg(8).wrap(len), None);
}

/// Pathfinder: the largest integer every double represents exactly. jq does
/// all arithmetic in doubles; within this bound an integer result is the same
/// either way, beyond it jq's rounding is reproduced by computing in `f64`.
const EXACT: isize = 1 << 53;

/// An integer operation as jq computes it: exact while the operands and the
/// result stay within [`EXACT`], in doubles otherwise.
fn int_as_jq(r: Option<isize>, x: isize, y: isize, op: fn(f64, f64) -> f64) -> Num {
    match r {
        Some(r) if x.abs() <= EXACT && y.abs() <= EXACT && r.abs() <= EXACT => Num::Int(r),
        _ => Num::Float(op(x as f64, y as f64)),
    }
}

fn int_or_big<const N: usize>(
    i: Option<isize>,
    x: [isize; N],
    f: fn([BigInt; N]) -> BigInt,
) -> Num {
    i.map_or_else(|| Num::big_int(f(x.map(BigInt::from))), Num::Int)
}

// Do not use `BigInt::parse_bytes`, because it accepts arbitrary underscores between digits.
// https://github.com/rust-num/num-bigint/issues/340
fn bigint_from_str_radix(s: &str, radix: u32) -> Option<BigInt> {
    use num_bigint::Sign::{Minus, Plus};
    let f = |c, sign| s.strip_prefix(c).map(|s| (sign, s));
    let (sign, num) = f('-', Minus).or_else(|| f('+', Plus)).unwrap_or((Plus, s));
    biguint_from_str_radix(num, radix).map(|bu| BigInt::from_biguint(sign, bu))
}

fn biguint_from_str_radix(s: &str, radix: u32) -> Option<BigUint> {
    assert!((2..=36).contains(&radix));
    if s.is_empty() {
        return None;
    }

    // normalize all characters to plain digit values
    let digits = s.bytes().map(|b| {
        Some(match b {
            b'0'..=b'9' => b - b'0',
            b'a'..=b'z' => b - b'a' + 10,
            b'A'..=b'Z' => b - b'A' + 10,
            _ => return None,
        })
        .filter(|d| *d < radix as u8)
    });
    BigUint::from_radix_be(&digits.collect::<Option<Vec<_>>>()?, radix)
}

impl core::ops::Add for Num {
    type Output = Num;
    fn add(self, rhs: Self) -> Self::Output {
        use Num::*;
        match (self, rhs) {
            (Int(x), Int(y)) => int_as_jq(x.checked_add(y), x, y, |x, y| x + y),
            (Int(i), BigInt(b)) | (BigInt(b), Int(i)) => Float(b.to_f64().unwrap() + i as f64),
            (Int(i), Float(f)) | (Float(f), Int(i)) => Float(f + i as f64),
            (BigInt(x), BigInt(y)) => Float(x.to_f64().unwrap() + y.to_f64().unwrap()),
            (BigInt(i), Float(f)) | (Float(f), BigInt(i)) => Float(f + i.to_f64().unwrap()),
            (Float(x), Float(y)) => Float(x + y),
            (Dec(n), r) => Self::from_dec_str(&n) + r,
            (l, Dec(n)) => l + Self::from_dec_str(&n),
        }
    }
}

impl core::ops::Sub for Num {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self::Output {
        use Num::*;
        match (self, rhs) {
            (Int(x), Int(y)) => int_as_jq(x.checked_sub(y), x, y, |x, y| x - y),
            (Int(i), BigInt(b)) => Float(i as f64 - b.to_f64().unwrap()),
            (BigInt(b), Int(i)) => Float(b.to_f64().unwrap() - i as f64),
            (BigInt(x), BigInt(y)) => Float(x.to_f64().unwrap() - y.to_f64().unwrap()),
            (Float(f), Int(i)) => Float(f - i as f64),
            (Int(i), Float(f)) => Float(i as f64 - f),
            (Float(f), BigInt(i)) => Float(f - i.to_f64().unwrap()),
            (BigInt(i), Float(f)) => Float(i.to_f64().unwrap() - f),
            (Float(x), Float(y)) => Float(x - y),
            (Dec(n), r) => Self::from_dec_str(&n) - r,
            (l, Dec(n)) => l - Self::from_dec_str(&n),
        }
    }
}

impl core::ops::Mul for Num {
    type Output = Self;
    fn mul(self, rhs: Self) -> Self::Output {
        use Num::*;
        match (self, rhs) {
            // A zero product with one negative factor is -0 in doubles.
            (Int(x), Int(y)) if (x == 0 || y == 0) && (x < 0) != (y < 0) => Float(-0.0),
            (Int(x), Int(y)) => int_as_jq(x.checked_mul(y), x, y, |x, y| x * y),

            (Int(i), BigInt(b)) | (BigInt(b), Int(i)) => Float(b.to_f64().unwrap() * i as f64),
            (BigInt(x), BigInt(y)) => Float(x.to_f64().unwrap() * y.to_f64().unwrap()),
            (BigInt(i), Float(f)) | (Float(f), BigInt(i)) => Float(f * i.to_f64().unwrap()),
            (Float(f), Int(i)) | (Int(i), Float(f)) => Float(f * i as f64),
            (Float(x), Float(y)) => Float(x * y),
            (Dec(n), r) => Self::from_dec_str(&n) * r,
            (l, Dec(n)) => l * Self::from_dec_str(&n),
        }
    }
}

impl core::ops::Div for Num {
    type Output = Self;
    fn div(self, rhs: Self) -> Self::Output {
        use Num::{BigInt, Dec, Float, Int};
        match (self, rhs) {
            (Int(l), r) => Float(l as f64) / r,
            (l, Int(r)) => l / Float(r as f64),
            (BigInt(l), r) => Float(l.to_f64().unwrap()) / r,
            (l, BigInt(r)) => l / Float(r.to_f64().unwrap()),
            (Float(x), Float(y)) => Float(x / y),
            (Dec(n), r) => Self::from_dec_str(&n) / r,
            (l, Dec(n)) => l / Self::from_dec_str(&n),
        }
    }
}

/// Pathfinder: jq's `dtoi`, which clamps a double to `intmax_t` before `%`.
///
/// Exactly 2^63 escapes both clamps and is converted by C's `(intmax_t)`,
/// which is undefined; x86-64's `cvttsd2si` (jq 1.8.1 as measured) yields
/// `i64::MIN` there, where Rust's `as` would saturate to `i64::MAX`.
fn dtoi(n: f64) -> i64 {
    const MIN: f64 = i64::MIN as f64;
    if n < MIN {
        i64::MIN
    } else if -n < MIN {
        i64::MAX
    } else if n >= -MIN {
        i64::MIN
    } else {
        n as i64
    }
}

impl Num {
    /// Pathfinder: the remainder as jq 1.8.1's `binop_mod` computes it: NaN
    /// if either side is NaN, else both sides are truncated to integers by
    /// [`dtoi`] (so `infinite % 1` is `0`, not NaN) and the remainder takes
    /// the dividend's sign. `None` when the truncated divisor is zero, which
    /// jq reports as an error.
    pub fn rem_jq(&self, rhs: &Self) -> Option<Self> {
        use Num::*;
        let (x, y) = match (self, rhs) {
            // Exact in doubles, so jq's conversion changes nothing.
            (Int(x), Int(y)) if x.abs() <= EXACT && y.abs() <= EXACT => {
                return x.checked_rem(*y).map(Int);
            }
            (x, y) => (x.as_f64(), y.as_f64()),
        };
        if x.is_nan() || y.is_nan() {
            return Some(Float(f64::NAN));
        }
        let (x, y) = (dtoi(x), dtoi(y));
        match y {
            0 => None,
            // `i64::MIN % -1` overflows; jq answers 0.
            -1 => Some(Int(0)),
            y => {
                let r = x % y;
                // jq's result is a double: beyond 2^53 it is rounded.
                Some(match isize::try_from(r) {
                    Ok(r) if r.abs() <= EXACT => Int(r),
                    _ => Float(r as f64),
                })
            }
        }
    }
}

impl core::ops::Rem for Num {
    type Output = Self;
    /// Pathfinder: [`Num::rem_jq`]; a zero divisor (guarded by `Val`) is NaN.
    fn rem(self, rhs: Self) -> Self::Output {
        self.rem_jq(&rhs).unwrap_or(Num::Float(f64::NAN))
    }
}

impl core::ops::Neg for Num {
    type Output = Self;
    fn neg(self) -> Self::Output {
        match self {
            // jq negates a literal exactly, so negation stays exact here.
            Self::Int(x) => int_or_big(x.checked_neg(), [x], |[x]| -x),
            Self::BigInt(x) => Self::big_int(-&*x),
            Self::Float(x) => Self::Float(-x),
            Self::Dec(n) => match n.strip_prefix('-') {
                Some(pos) => Self::Dec(pos.to_string().into()),
                None => Self::Dec(alloc::format!("-{n}").into()),
            },
        }
    }
}

impl Hash for Num {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            // hash machine-sized integers like floating-point numbers,
            // because they are also compared for equality that way
            Self::Int(i) => Self::Float(*i as f64).hash(state),
            // hash all non-finite floats, like NaN and infinity, to the same
            // note that `Val::hash` assumes that `Num::hash` always starts
            // with `state.write_u8(n)`, where `n < 2`
            Self::Float(f) => {
                state.write_u8(0);
                if f.is_finite() {
                    f.to_ne_bytes().hash(state);
                }
            }
            Self::Dec(d) => Self::from_dec_str(d).hash(state),
            Self::BigInt(i) => {
                let f = i.to_f64().unwrap();
                if f.is_finite() {
                    Self::Float(f).hash(state)
                } else {
                    state.write_u8(1);
                    i.hash(state)
                }
            }
        }
    }
}

#[test]
fn hash_nums() {
    use core::f64::consts::PI;
    use core::hash::BuildHasher;
    let h = |n| foldhash::fast::FixedState::with_seed(42).hash_one(n);

    assert_eq!(h(Num::Int(4096)), h(Num::big_int(4096.into())));
    assert_eq!(h(Num::Float(0.0)), h(Num::Int(0)));
    assert_eq!(h(Num::Float(3.0)), h(Num::big_int(3.into())));

    assert_ne!(h(Num::Float(0.2)), h(Num::Float(0.4)));
    assert_ne!(h(Num::Float(PI)), h(Num::big_int(3.into())));
    assert_ne!(h(Num::Float(0.2)), h(Num::Int(1)));
    assert_ne!(h(Num::Int(1)), h(Num::Int(2)));
}

impl PartialEq for Num {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Int(x), Self::Int(y)) => x == y,
            (Self::Int(i), Self::Float(f)) | (Self::Float(f), Self::Int(i)) => {
                f.is_finite() && float_eq(*i as f64, *f)
            }
            (Self::BigInt(x), Self::BigInt(y)) => x == y,
            (Self::Int(i), Self::BigInt(b)) | (Self::BigInt(b), Self::Int(i)) => {
                **b == BigInt::from(*i)
            }
            (Self::BigInt(i), Self::Float(f)) | (Self::Float(f), Self::BigInt(i)) => {
                f.is_finite() && float_eq(i.to_f64().unwrap(), *f)
            }
            (Self::Float(x), Self::Float(y)) => float_eq(*x, *y),
            (Self::Dec(x), Self::Dec(y)) if Rc::ptr_eq(x, y) => true,
            (Self::Dec(n), y) => &Self::from_dec_str(n) == y,
            (x, Self::Dec(n)) => x == &Self::from_dec_str(n),
        }
    }
}

impl Eq for Num {}

impl PartialOrd for Num {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Num {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::Int(x), Self::Int(y)) => x.cmp(y),
            (Self::Int(x), Self::BigInt(y)) => BigInt::from(*x).cmp(y),
            (Self::Int(i), Self::Float(f)) => float_cmp(*i as f64, *f),
            (Self::BigInt(x), Self::Int(y)) => (**x).cmp(&BigInt::from(*y)),
            (Self::BigInt(x), Self::BigInt(y)) => x.cmp(y),
            // BigInt::to_f64 always yields Some, large values become f64::INFINITY
            (Self::BigInt(x), Self::Float(y)) => float_cmp(x.to_f64().unwrap(), *y),
            (Self::Float(f), Self::Int(i)) => float_cmp(*f, *i as f64),
            (Self::Float(x), Self::BigInt(y)) => float_cmp(*x, y.to_f64().unwrap()),
            (Self::Float(x), Self::Float(y)) => float_cmp(*x, *y),
            (Self::Dec(x), Self::Dec(y)) if Rc::ptr_eq(x, y) => Ordering::Equal,
            (Self::Dec(n), y) => Self::from_dec_str(n).cmp(y),
            (x, Self::Dec(n)) => x.cmp(&Self::from_dec_str(n)),
        }
    }
}

fn float_eq(left: f64, right: f64) -> bool {
    float_cmp(left, right) == Ordering::Equal
}

fn float_cmp(left: f64, right: f64) -> Ordering {
    if left == 0. && right == 0. {
        // consider negative and positive 0 as equal
        Ordering::Equal
    } else if left.is_nan() {
        // there are more than 50 shades of NaN, and which of these
        // you strike when you perform a calculation is not deterministic (!),
        // therefore `total_cmp` may yield different results for the same calculation
        // so we bite the bullet and handle this like in jq
        Ordering::Less
    } else if right.is_nan() {
        Ordering::Greater
    } else {
        f64::total_cmp(&left, &right)
    }
}

// Pathfinder: numbers are printed as jq 1.8.1 prints them. A float goes
// through jq's `jvp_dtoa_fmt` layout (shortest round-trip digits; `1e+17`,
// `1e-05`, no `.0` on an integral value); NaN is `null` and an infinity is
// the largest finite double, which keeps the output JSON. A decimal literal
// keeps its digits but is spelled the way jq's decNumber spells it (`1E+3`
// for `1e3`, `1.000` stays `1.000`).
impl fmt::Display for Num {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Int(i) => write!(f, "{i}"),
            Self::BigInt(i) => write!(f, "{i}"),
            Self::Float(x) if x.is_nan() => write!(f, "null"),
            Self::Float(x) => f.write_str(&jq_float(x.clamp(f64::MIN, f64::MAX))),
            // Most literals are already canonical: no exponent, and no
            // leading zero to fold. Those are written through unchanged.
            Self::Dec(n) if dec_is_canonical(n) => f.write_str(n),
            Self::Dec(n) => f.write_str(&jq_dec(n).unwrap_or_else(|| n.to_string())),
        }
    }
}

/// jq's `jvp_dtoa_fmt`: David Gay's `g_fmt` over the shortest digits.
fn jq_float(x: f64) -> String {
    // `{:e}` is the shortest round-trip representation, like `dtoa` mode 0.
    let sci = alloc::format!("{x:e}");
    let (mantissa, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let (sign, mantissa) = match mantissa.strip_prefix('-') {
        Some(m) => ("-", m),
        None => ("", mantissa),
    };
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let decpt = exp.parse::<i32>().unwrap_or(0) + 1;
    let n = digits.len() as i32;
    let mut out = String::from(sign);
    if decpt <= -4 || decpt > n + 15 {
        out.push_str(&digits[..1]);
        if n > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        let e = decpt - 1;
        out.push_str(&alloc::format!(
            "e{}{:02}",
            if e < 0 { '-' } else { '+' },
            e.abs()
        ));
    } else if decpt <= 0 {
        out.push_str("0.");
        out.extend(core::iter::repeat_n('0', (-decpt) as usize));
        out.push_str(&digits);
    } else if decpt >= n {
        out.push_str(&digits);
        out.extend(core::iter::repeat_n('0', (decpt - n) as usize));
    } else {
        out.push_str(&digits[..decpt as usize]);
        out.push('.');
        out.push_str(&digits[decpt as usize..]);
    }
    out
}

/// Whether decNumber would print the literal `n` exactly as written: plain
/// notation (no exponent) whose integer part is non-zero and has no leading
/// zero, or `0.` followed by fewer than six zeros before the first digit.
fn dec_is_canonical(n: &str) -> bool {
    let n = n.strip_prefix('-').unwrap_or(n);
    let b = n.as_bytes();
    if b.iter().any(|c| matches!(c, b'e' | b'E' | b'+')) || b.is_empty() {
        return false;
    }
    match b {
        [b'0', b'.', frac @ ..] => {
            let zeros = frac.iter().take_while(|c| **c == b'0').count();
            zeros < 6 && zeros < frac.len()
        }
        [b'1'..=b'9', ..] => true,
        _ => false,
    }
}

/// decNumber's to-scientific-string for a decimal literal, or `None` if the
/// text is not one.
fn jq_dec(n: &str) -> Option<String> {
    let (sign, n) = match n.strip_prefix('-') {
        Some(rest) => ("-", rest),
        None => ("", n.strip_prefix('+').unwrap_or(n)),
    };
    let (mantissa, exp) = match n.find(['e', 'E']) {
        Some(i) => (&n[..i], n[i + 1..].parse::<i64>().ok()?),
        None => (n, 0),
    };
    let (int, frac) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    if int.is_empty() && frac.is_empty()
        || !(int.bytes().chain(frac.bytes()).all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    let all: String = alloc::format!("{int}{frac}");
    let coeff = match all.trim_start_matches('0') {
        "" => "0",
        c => c,
    };
    let e = exp.checked_sub(i64::try_from(frac.len()).ok()?)?;
    let len = coeff.len() as i64;
    let adjusted = e + len - 1;
    let mut out = String::from(sign);
    if e <= 0 && adjusted >= -6 {
        if e == 0 {
            out.push_str(coeff);
        } else if len > -e {
            let point = (len + e) as usize;
            out.push_str(&coeff[..point]);
            out.push('.');
            out.push_str(&coeff[point..]);
        } else {
            out.push_str("0.");
            out.extend(core::iter::repeat_n('0', (-e - len) as usize));
            out.push_str(coeff);
        }
    } else {
        out.push_str(&coeff[..1]);
        if len > 1 {
            out.push('.');
            out.push_str(&coeff[1..]);
        }
        out.push_str(&alloc::format!(
            "E{}{}",
            if adjusted < 0 { '-' } else { '+' },
            adjusted.abs()
        ));
    }
    Some(out)
}
