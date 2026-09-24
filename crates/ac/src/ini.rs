//! Minimal INI reader: `[SECTION]` headers, `KEY=VALUE` lines, `;` and `//` comments.

#[derive(Clone, Debug, Default)]
pub struct Section {
    pub name: String,
    pub entries: Vec<(String, String)>,
}

impl Section {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
    }

    pub fn get_f64(&self, key: &str) -> Option<f64> {
        self.get(key)?.parse().ok()
    }

    /// A comma-separated list of numbers, e.g. `0.1, 0.5, -1`.
    pub fn get_f64s(&self, key: &str) -> Option<Vec<f64>> {
        self.get(key)?
            .split(',')
            .map(|v| v.trim().parse().ok())
            .collect()
    }
}

/// The first section called `name`, compared case-insensitively.
pub fn section<'a>(sections: &'a [Section], name: &str) -> Option<&'a Section> {
    sections.iter().find(|s| s.name.eq_ignore_ascii_case(name))
}

pub fn parse(src: &str) -> Vec<Section> {
    let mut sections: Vec<Section> = Vec::new();
    for line in src.lines() {
        let line = line.split(';').next().unwrap_or("");
        let line = line.split("//").next().unwrap_or("").trim();
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            sections.push(Section {
                name: name.trim().to_string(),
                entries: Vec::new(),
            });
        } else if let Some((k, v)) = line.split_once('=')
            && let Some(section) = sections.last_mut()
        {
            section
                .entries
                .push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    sections
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections_keys_and_comments() {
        let s = parse(
            "; header\n[SURFACE_0]\nKEY=ROAD ; main\nFRICTION = 0.97\n\n[MODEL_0]\nFILE=a.kn5 // x\n",
        );
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].get("key"), Some("ROAD"));
        assert_eq!(s[0].get_f64("FRICTION"), Some(0.97));
        assert_eq!(s[1].get("FILE"), Some("a.kn5"));
        let s = parse(
            "[A]
POS=0.5, -1,2
BAD=1,x
",
        );
        assert_eq!(
            section(&s, "a").unwrap().get_f64s("POS"),
            Some(vec![0.5, -1.0, 2.0])
        );
        assert_eq!(s[0].get_f64s("BAD"), None);
    }
}
