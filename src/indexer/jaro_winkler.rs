/// Port of `DidYouMean::Jaro` and `DidYouMean::JaroWinkler`, which Ruby LSP uses for fuzzy searching the index.
fn jaro_distance(first: &[char], second: &[char]) -> f64 {
    let (shorter, longer) = if first.len() > second.len() { (second, first) } else { (first, second) };
    let length1 = shorter.len();
    let length2 = longer.len();

    let mut matches = 0.0f64;
    let mut transpositions = 0.0f64;
    let range = (length2 / 2).saturating_sub(1);
    let mut flags1 = vec![false; length1];
    let mut flags2 = vec![false; length2];

    for i in 0..length1 {
        let last = i + range;
        let mut j = i.saturating_sub(range);
        while j <= last {
            if j < length2 && !flags2[j] && shorter[i] == longer[j] {
                flags2[j] = true;
                flags1[i] = true;
                matches += 1.0;
                break;
            }
            j += 1;
        }
    }

    let mut k = 0;
    for i in 0..length1 {
        if flags1[i] {
            let mut j = k;
            let mut index = k;
            let mut next_k = None;
            while j < length2 {
                index = j;
                if flags2[j] {
                    next_k = Some(j + 1);
                    break;
                }
                j += 1;
            }
            k = next_k.unwrap_or(0);
            if longer.get(index) != Some(&shorter[i]) {
                transpositions += 1.0;
            }
        }
    }
    let transpositions = (transpositions / 2.0).floor();

    if matches == 0.0 {
        0.0
    } else {
        (matches / length1 as f64 + matches / length2 as f64 + (matches - transpositions) / matches) / 3.0
    }
}

pub fn distance(first: &str, second: &str) -> f64 {
    let first: Vec<char> = first.chars().collect();
    let second: Vec<char> = second.chars().collect();
    let jaro = jaro_distance(&first, &second);

    if jaro > 0.7 {
        let mut prefix_bonus = 0;
        for (i, character) in first.iter().enumerate() {
            if second.get(i) == Some(character) && i < 4 {
                prefix_bonus += 1;
            } else {
                break;
            }
        }
        jaro + (prefix_bonus as f64 * 0.1 * (1.0 - jaro))
    } else {
        jaro
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_did_you_mean_values() {
        assert!((distance("martha", "marhta") - 0.9611111111111111).abs() < 1e-9);
        assert_eq!(distance("abc", "xyz"), 0.0);
    }
}
