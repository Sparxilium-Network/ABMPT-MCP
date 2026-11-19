//! Line-preserving editing of `server.properties`.

/// Parse `key=value` pairs (comments and blank lines ignored).
pub fn parse(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|l| {
            let t = l.trim_start();
            if t.is_empty() || t.starts_with('#') || t.starts_with('!') {
                return None;
            }
            let (k, v) = t.split_once(['=', ':']).unwrap_or((t, ""));
            Some((k.trim().to_string(), v.trim_start().to_string()))
        })
        .collect()
}

pub fn get(text: &str, key: &str) -> Option<String> {
    parse(text).into_iter().find(|(k, _)| k == key).map(|(_, v)| v)
}

/// Set keys in place, appending missing ones. Other lines are kept untouched.
pub fn set_many(text: &str, kv: &[(&str, String)]) -> String {
    let mut done = vec![false; kv.len()];
    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        let t = line.trim_start();
        let key = if t.starts_with('#') || t.starts_with('!') || t.is_empty() {
            None
        } else {
            Some(t.split_once(['=', ':']).map(|(k, _)| k).unwrap_or(t).trim())
        };
        match key.and_then(|k| kv.iter().position(|(kk, _)| *kk == k)) {
            Some(i) if !done[i] => {
                out.push(format!("{}={}", kv[i].0, kv[i].1));
                done[i] = true;
            }
            Some(_) => {} // drop duplicate definitions of managed keys
            None => out.push(line.to_string()),
        }
    }
    for (i, (k, v)) in kv.iter().enumerate() {
        if !done[i] {
            out.push(format!("{k}={v}"));
        }
    }
    let mut s = out.join("\n");
    s.push('\n');
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_preserves_other_lines() {
        let src = "#Minecraft\nmotd=hi\nserver-port=1\nserver-port=2\nlevel-seed=\n";
        let out = set_many(src, &[("server-port", "65000".into()), ("rcon.port", "35000".into())]);
        assert_eq!(out, "#Minecraft\nmotd=hi\nserver-port=65000\nlevel-seed=\nrcon.port=35000\n");
        assert_eq!(get(&out, "server-port").as_deref(), Some("65000"));
        assert_eq!(get(&out, "level-seed").as_deref(), Some(""));
    }
}
