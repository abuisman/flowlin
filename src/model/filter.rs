//! Filter entry syntax: free text matches names (case-insensitive substring);
//! `rating:>=3`, `rating:5`, `rating:<2` filter by star rating.

use super::ImageItem;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cmp {
    Eq,
    Ge,
    Le,
    Gt,
    Lt,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FilterSpec {
    pub text: String,
    pub needle: String,
    pub rating: Option<(Cmp, u8)>,
}

impl FilterSpec {
    pub fn is_empty(&self) -> bool {
        self.needle.is_empty() && self.rating.is_none()
    }

    pub fn matches_parts(&self, name: &str, rating: u8) -> bool {
        if !self.needle.is_empty() && !name.to_lowercase().contains(&self.needle) {
            return false;
        }
        if let Some((cmp, v)) = self.rating {
            let ok = match cmp {
                Cmp::Eq => rating == v,
                Cmp::Ge => rating >= v,
                Cmp::Le => rating <= v,
                Cmp::Gt => rating > v,
                Cmp::Lt => rating < v,
            };
            if !ok {
                return false;
            }
        }
        true
    }

    pub fn matches(&self, item: &ImageItem) -> bool {
        if self.is_empty() {
            return true;
        }
        let rating = if self.rating.is_some() {
            match item.rating() {
                Some(r) => r,
                None => {
                    let r = item.with_path(crate::fs::xattrs::read_rating);
                    item.set_rating(Some(r));
                    r
                }
            }
        } else {
            0
        };
        item.with_name(|n| self.matches_parts(n, rating))
    }
}

pub fn parse_filter(text: &str) -> FilterSpec {
    let mut needle = Vec::new();
    let mut rating = None;
    for tok in text.split_whitespace() {
        if let Some(rest) = tok.strip_prefix("rating:") {
            let (cmp, num) = if let Some(n) = rest.strip_prefix(">=") {
                (Cmp::Ge, n)
            } else if let Some(n) = rest.strip_prefix("<=") {
                (Cmp::Le, n)
            } else if let Some(n) = rest.strip_prefix('>') {
                (Cmp::Gt, n)
            } else if let Some(n) = rest.strip_prefix('<') {
                (Cmp::Lt, n)
            } else if let Some(n) = rest.strip_prefix('=') {
                (Cmp::Eq, n)
            } else {
                (Cmp::Eq, rest)
            };
            if let Ok(v) = num.parse::<u8>() {
                rating = Some((cmp, v.min(5)));
                continue;
            }
        }
        needle.push(tok);
    }
    FilterSpec { text: text.to_string(), needle: needle.join(" ").to_lowercase(), rating }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substring_case_insensitive() {
        let f = parse_filter("BeAch");
        assert!(f.matches_parts("Sunny_beach.JPG", 0));
        assert!(!f.matches_parts("mountain.jpg", 0));
    }

    #[test]
    fn rating_expressions() {
        let f = parse_filter("rating:>=3");
        assert_eq!(f.rating, Some((Cmp::Ge, 3)));
        assert!(f.needle.is_empty());
        assert!(f.matches_parts("x", 3));
        assert!(!f.matches_parts("x", 2));
        let f = parse_filter("rating:5 cat");
        assert!(f.matches_parts("Cat.png", 5));
        assert!(!f.matches_parts("Cat.png", 4));
        assert!(!f.matches_parts("dog.png", 5));
        assert!(parse_filter("rating:<1").matches_parts("a", 0));
        assert!(parse_filter("").is_empty());
    }
}
