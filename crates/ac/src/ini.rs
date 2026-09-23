//! Minimal INI reader: `[SECTION]` headers, `KEY=VALUE` lines, `;` and `//` comments.

#[derive(Clone, Debug, Default)]
pub struct Section {
    pub name: String,
    pub entries: Vec<(String, String)>,
}

impl Section {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v.as_str())
    }

    pub fn get_f64(&self, key: &str) -> Option<f64> {
        self.get(key)?.parse().ok()
    }
}

pub fn parse(src: &str) -> Vec<Section> {
    let mut sections: Vec<Section> = Vec::new();
    for line in src.lines() {
        let line = line.split(';').next().unwrap_or("");
        let line = line.split("//").next().unwrap_or("").trim();
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            sections.push(Section { name: name.trim().to_string(), entries: Vec::new() });
        } else if let Some((k, v)) = line.split_once('=')
            && let Some(section) = sections.last_mut()
        {
            section.entries.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    sections
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections_keys_and_comments() {
        let s = parse("; header\n[SURFACE_0]\nKEY=ROAD ; main\nFRICTION = 0.97\n\n[MODEL_0]\nFILE=a.kn5 // x\n");
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].get("key"), Some("ROAD"));
        assert_eq!(s[0].get_f64("FRICTION"), Some(0.97));
        assert_eq!(s[1].get("FILE"), Some("a.kn5"));
    }
}
