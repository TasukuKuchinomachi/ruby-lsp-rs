use ruby_prism::{Integer, Node as PrismNode};

use crate::ruby::node::Child;

pub(super) enum NumericValue {
    Integer(String),
    Float(f64),
    Rational {
        numerator: String,
        denominator: String,
    },
    Imaginary(Box<NumericValue>),
}

fn decimal(integer: Integer<'_>) -> String {
    let (negative, digits) = integer.to_u32_digits();
    let mut words: Vec<u32> = digits.to_vec();
    let mut chunks: Vec<u32> = Vec::new();
    while words.iter().any(|word| *word != 0) {
        let mut remainder: u64 = 0;
        for word in words.iter_mut().rev() {
            let value = (remainder << 32) | u64::from(*word);
            *word = (value / 1_000_000_000) as u32;
            remainder = value % 1_000_000_000;
        }
        chunks.push(remainder as u32);
    }
    let mut text = match chunks.pop() {
        None => "0".to_string(),
        Some(first) => first.to_string(),
    };
    for chunk in chunks.iter().rev() {
        text.push_str(&format!("{chunk:09}"));
    }
    if negative && text != "0" {
        text.insert(0, '-');
    }
    text
}

fn negate_decimal(text: &str) -> String {
    match text.strip_prefix('-') {
        Some(positive) => positive.to_string(),
        None if text == "0" => text.to_string(),
        None => format!("-{text}"),
    }
}

pub fn inspect_float(value: f64) -> String {
    if value.is_nan() {
        return "NaN".to_string();
    }
    if value.is_infinite() {
        return if value > 0.0 { "Infinity" } else { "-Infinity" }.to_string();
    }
    if value == 0.0 {
        return if value.is_sign_negative() {
            "-0.0"
        } else {
            "0.0"
        }
        .to_string();
    }
    let scientific = format!("{:e}", value.abs());
    let (mantissa, exponent) = scientific
        .split_once('e')
        .expect("the scientific notation has an exponent");
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let exponent: i32 = exponent.parse().expect("the exponent is an integer");
    let decimal_point = exponent + 1;
    let sign = if value < 0.0 { "-" } else { "" };
    let digit_count = digits.len() as i32;
    if 0 < decimal_point && decimal_point < 16 {
        if decimal_point >= digit_count {
            let zeros = "0".repeat((decimal_point - digit_count) as usize);
            format!("{sign}{digits}{zeros}.0")
        } else {
            let (integer_part, fraction_part) = digits.split_at(decimal_point as usize);
            format!("{sign}{integer_part}.{fraction_part}")
        }
    } else if -4 < decimal_point && decimal_point <= 0 {
        let zeros = "0".repeat((-decimal_point) as usize);
        format!("{sign}0.{zeros}{digits}")
    } else {
        let (first, rest) = digits.split_at(1);
        let rest = if rest.is_empty() { "0" } else { rest };
        let exponent = decimal_point - 1;
        let exponent_sign = if exponent < 0 { '-' } else { '+' };
        format!("{sign}{first}.{rest}e{exponent_sign}{:02}", exponent.abs())
    }
}

impl NumericValue {
    pub fn from_node(node: &PrismNode<'_>) -> Option<NumericValue> {
        if let Some(integer) = node.as_integer_node() {
            Some(NumericValue::Integer(decimal(integer.value())))
        } else if let Some(float) = node.as_float_node() {
            Some(NumericValue::Float(float.value()))
        } else if let Some(rational) = node.as_rational_node() {
            Some(NumericValue::Rational {
                numerator: decimal(rational.numerator()),
                denominator: decimal(rational.denominator()),
            })
        } else if let Some(imaginary) = node.as_imaginary_node() {
            NumericValue::from_node(&imaginary.numeric())
                .map(|numeric| NumericValue::Imaginary(Box::new(numeric)))
        } else {
            None
        }
    }

    pub fn negate(self) -> NumericValue {
        match self {
            NumericValue::Integer(value) => NumericValue::Integer(negate_decimal(&value)),
            NumericValue::Float(value) => NumericValue::Float(-value),
            NumericValue::Rational {
                numerator,
                denominator,
            } => NumericValue::Rational {
                numerator: negate_decimal(&numerator),
                denominator,
            },
            NumericValue::Imaginary(value) => NumericValue::Imaginary(Box::new(value.negate())),
        }
    }

    fn inspect(&self) -> String {
        match self {
            NumericValue::Integer(value) => value.clone(),
            NumericValue::Float(value) => inspect_float(*value),
            NumericValue::Rational {
                numerator,
                denominator,
            } => format!("({numerator}/{denominator})"),
            NumericValue::Imaginary(value) => {
                let (positive, absolute) = match value.as_ref() {
                    NumericValue::Integer(value) => (
                        !value.starts_with('-'),
                        value.trim_start_matches('-').to_string(),
                    ),
                    NumericValue::Float(value) => (
                        value.is_nan() || !value.is_sign_negative(),
                        inspect_float(value.abs()),
                    ),
                    NumericValue::Rational {
                        numerator,
                        denominator,
                    } => (
                        !numerator.starts_with('-'),
                        format!("({}/{denominator})", numerator.trim_start_matches('-')),
                    ),
                    NumericValue::Imaginary(_) => (true, value.inspect()),
                };
                let mut text = format!("(0{}{absolute}", if positive { '+' } else { '-' });
                if !text.ends_with(|character: char| character.is_ascii_digit()) {
                    text.push('*');
                }
                text.push_str("i)");
                text
            }
        }
    }

    pub fn child(&self) -> (&'static str, Child) {
        match self {
            NumericValue::Integer(value) => ("int", Child::Integer(value.clone())),
            NumericValue::Float(value) => ("float", Child::Float(*value)),
            NumericValue::Rational { .. } => ("rational", Child::Rational(self.inspect())),
            NumericValue::Imaginary(_) => ("complex", Child::Complex(self.inspect())),
        }
    }
}
