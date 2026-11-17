//! Web helpers: URL domain extraction, robots.txt evaluation, Retry-After parsing.

pub fn host_of(url: &str) -> Option<String> {
    let u = url.trim();
    let rest = u.split_once("://").map(|x| x.1).unwrap_or(u);
    let auth = rest.split(['/', '?', '#']).next()?;
    let auth = auth.rsplit('@').next()?;
    let host = if auth.starts_with('[') { auth.split(']').next()?.trim_start_matches('[') } else { auth.split(':').next()? };
    let h = host.trim().trim_end_matches('.').to_lowercase();
    if h.is_empty() || !h.contains('.') && h != "localhost" {
        return None;
    }
    Some(h)
}

/// Registrable-domain heuristic: "a.b.example.co.uk" -> "example.co.uk"; "www.x.com" -> "x.com".
pub fn domain_of(url: &str) -> Option<String> {
    let h = host_of(url)?;
    if h.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return Some(h);
    }
    let parts: Vec<&str> = h.split('.').collect();
    if parts.len() <= 2 {
        return Some(h);
    }
    let n = parts.len();
    let second = parts[n - 2];
    let take = if parts[n - 1].len() == 2 && matches!(second, "co" | "com" | "org" | "net" | "gov" | "edu" | "ac" | "or" | "ne") { 3 } else { 2 };
    Some(parts[n - take..].join("."))
}

fn glob_match(pat: &[u8], s: &[u8]) -> bool {
    // '*' matches any run, trailing '$' anchors the end; everything else is a literal prefix match
    let (pat, anchored) = if pat.last() == Some(&b'$') { (&pat[..pat.len() - 1], true) } else { (pat, false) };
    let (mut p, mut i) = (0usize, 0usize);
    let mut star: Option<(usize, usize)> = None;
    while i < s.len() || p < pat.len() {
        if p < pat.len() && pat[p] == b'*' {
            star = Some((p, i));
            p += 1;
            continue;
        }
        if p == pat.len() && !anchored {
            return true;
        }
        if p < pat.len() && i < s.len() && pat[p] == s[i] {
            p += 1;
            i += 1;
            continue;
        }
        match star {
            Some((sp, si)) if si < s.len() => {
                star = Some((sp, si + 1));
                p = sp + 1;
                i = si + 1;
            }
            _ => return p == pat.len() && (!anchored || i == s.len()) || (p == pat.len() && !anchored),
        }
    }
    true
}

/// Returns (allowed, crawl_delay_seconds). The most specific user-agent group wins; the longest matching rule
/// wins, and Allow beats Disallow on equal length.
pub fn robots(txt: &str, agent: &str, path: &str) -> (bool, Option<f64>) {
    let agent = agent.to_lowercase();
    let mut groups: Vec<(Vec<String>, Vec<(bool, String)>, Option<f64>)> = Vec::new();
    let mut cur_agents: Vec<String> = Vec::new();
    let mut in_rules = false;
    for line in txt.lines() {
        let l = line.split('#').next().unwrap_or("").trim();
        let Some((k, v)) = l.split_once(':') else { continue };
        let (k, v) = (k.trim().to_lowercase(), v.trim());
        match k.as_str() {
            "user-agent" => {
                if in_rules {
                    cur_agents.clear();
                    in_rules = false;
                }
                cur_agents.push(v.to_lowercase());
                groups.push((cur_agents.clone(), Vec::new(), None));
            }
            "allow" | "disallow" => {
                in_rules = true;
                if let Some(g) = groups.last_mut() {
                    if !v.is_empty() {
                        g.1.push((k == "allow", v.to_string()));
                    }
                }
            }
            "crawl-delay" => {
                in_rules = true;
                if let Some(g) = groups.last_mut() {
                    g.2 = v.parse().ok();
                }
            }
            _ => {}
        }
    }
    let mut pick: Option<usize> = None;
    let mut best = 0usize;
    for (i, g) in groups.iter().enumerate() {
        for a in &g.0 {
            if a != "*" && agent.contains(a.as_str()) && a.len() > best {
                best = a.len();
                pick = Some(i);
            }
        }
    }
    if pick.is_none() {
        pick = groups.iter().position(|g| g.0.iter().any(|a| a == "*"));
    }
    let Some(gi) = pick else { return (true, None) };
    let g = &groups[gi];
    let mut verdict = (true, 0usize);
    for (allow, pat) in &g.1 {
        if glob_match(pat.as_bytes(), path.as_bytes()) {
            let len = pat.len();
            if len > verdict.1 || (len == verdict.1 && *allow) {
                verdict = (*allow, len);
            }
        }
    }
    (verdict.0, g.2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        assert_eq!(domain_of("https://www.Example.com:8080/a?b#c").unwrap(), "example.com");
        assert_eq!(domain_of("http://news.bbc.co.uk/x").unwrap(), "bbc.co.uk");
        assert_eq!(domain_of("https://user:pw@sub.site.org/").unwrap(), "site.org");
        assert_eq!(domain_of("not a url"), None);
    }

    #[test]
    fn robots_rules() {
        let t = "User-agent: *\nDisallow: /private\nAllow: /private/open\nCrawl-delay: 2\n\nUser-agent: mnxbot\nDisallow: /\n";
        assert_eq!(robots(t, "OtherBot/1.0", "/index.html"), (true, Some(2.0)));
        assert!(!robots(t, "OtherBot/1.0", "/private/x").0);
        assert!(robots(t, "OtherBot/1.0", "/private/open/file").0);
        assert!(!robots(t, "MNXBot/1.0", "/anything").0);
        let t2 = "User-agent: *\nDisallow: /*.pdf$\nDisallow: /tmp*\n";
        assert!(!robots(t2, "x", "/a/b.pdf").0);
        assert!(robots(t2, "x", "/a/b.pdf.html").0);
        assert!(!robots(t2, "x", "/tmp123").0);
        assert!(robots("", "x", "/a").0);
    }
}
