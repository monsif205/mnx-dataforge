//! License Gate: normalizes license names/URLs to identifiers and decides allow / review / deny for use as
//! training data. This is a policy helper, not legal advice; "review" always needs a human decision.

use crate::json::Value;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Cat {
    PublicDomain,
    Permissive,
    Attribution,
    ShareAlike,
    Copyleft,
    NonCommercial,
    NoDerivatives,
    Proprietary,
    Owner,
}

fn cc_info(by: bool, nc: bool, nd: bool, sa: bool, version: &str) -> Option<(String, Cat)> {
    if !by {
        return None;
    }
    let mut id = String::from("CC-BY");
    if nc {
        id.push_str("-NC");
    }
    if nd {
        id.push_str("-ND");
    }
    if sa {
        id.push_str("-SA");
    }
    if !version.is_empty() {
        id.push('-');
        id.push_str(version);
    }
    let cat = if nd {
        Cat::NoDerivatives
    } else if nc {
        Cat::NonCommercial
    } else if sa {
        Cat::ShareAlike
    } else {
        Cat::Attribution
    };
    Some((id, cat))
}

fn version_in(s: &str) -> String {
    let b: Vec<char> = s.chars().collect();
    for i in 0..b.len().saturating_sub(2) {
        if b[i].is_ascii_digit() && b[i + 1] == '.' && b[i + 2].is_ascii_digit() {
            return format!("{}.{}", b[i], b[i + 2]);
        }
    }
    String::new()
}

fn has_word(s: &str, w: &str) -> bool {
    s.split(|c: char| !c.is_ascii_alphanumeric()).any(|t| t == w)
}

fn classify(raw: &str) -> Option<(String, Cat)> {
    let s = raw.trim().to_lowercase();
    if s.is_empty() {
        return None;
    }
    let simple: &[(&str, &str, Cat)] = &[
        ("cc0", "CC0-1.0", Cat::PublicDomain),
        ("creativecommons.org/publicdomain/zero", "CC0-1.0", Cat::PublicDomain),
        ("publicdomain/mark", "CC-PDM-1.0", Cat::PublicDomain),
        ("public domain", "Public-Domain", Cat::PublicDomain),
        ("unlicense", "Unlicense", Cat::PublicDomain),
        ("pddl", "PDDL-1.0", Cat::PublicDomain),
        ("odc-by", "ODC-By-1.0", Cat::Attribution),
        ("odbl", "ODbL-1.0", Cat::ShareAlike),
        ("apache", "Apache-2.0", Cat::Permissive),
        ("mit license", "MIT", Cat::Permissive),
        ("opensource.org/licenses/mit", "MIT", Cat::Permissive),
        ("bsd", "BSD", Cat::Permissive),
        ("isc license", "ISC", Cat::Permissive),
        ("zlib", "Zlib", Cat::Permissive),
        ("open government licence", "OGL-3.0", Cat::Attribution),
        ("ogl-uk", "OGL-3.0", Cat::Attribution),
        ("agpl", "AGPL-3.0", Cat::Copyleft),
        ("lgpl", "LGPL", Cat::Copyleft),
        ("gpl", "GPL", Cat::Copyleft),
        ("mozilla public", "MPL-2.0", Cat::Copyleft),
        ("mpl-2", "MPL-2.0", Cat::Copyleft),
        ("all rights reserved", "All-Rights-Reserved", Cat::Proprietary),
        ("proprietary", "Proprietary", Cat::Proprietary),
        ("copyright", "Copyright", Cat::Proprietary),
    ];
    let is_cc = s.contains("creativecommons.org/licenses") || s.contains("cc-by") || s.contains("cc by") || s.contains("cc_by") || s.contains("creative commons") || s.contains("attribution");
    if is_cc {
        let toks: Vec<&str> = s.split(|c: char| !c.is_ascii_alphanumeric()).filter(|t| !t.is_empty()).collect();
        let nc = toks.contains(&"nc") || toks.contains(&"noncommercial") || s.contains("non-commercial") || s.contains("noncommercial");
        let nd = toks.contains(&"nd") || toks.contains(&"noderivatives") || toks.contains(&"noderivs") || s.contains("no-derivs") || s.contains("noderiv");
        let sa = toks.contains(&"sa") || toks.contains(&"sharealike") || s.contains("share-alike") || s.contains("sharealike");
        let by = toks.contains(&"by") || s.contains("attribution");
        if let Some((id, cat)) = cc_info(by, nc, nd, sa, &version_in(&s)) {
            return Some((id, cat));
        }
    }
    for (pat, id, cat) in simple {
        if s.contains(pat) {
            let v = version_in(&s);
            let name = if !v.is_empty() && matches!(*id, "GPL" | "LGPL") { format!("{}-{}", id, v) } else { id.to_string() };
            return Some((name, *cat));
        }
    }
    if has_word(&s, "mit") {
        return Some(("MIT".into(), Cat::Permissive));
    }
    if has_word(&s, "owner") || s == "own" || s == "mine" || s.contains("i own") || s.contains("my own") {
        return Some(("Owner".into(), Cat::Owner));
    }
    if s.contains("permission") || s.contains("licensed to me") || s.contains("agreement") || s.contains("contract") {
        return Some(("Permission".into(), Cat::Owner));
    }
    None
}

fn flags(cat: Cat) -> (bool, bool, bool, bool) {
    // (commercial_ok, attribution_required, share_alike, no_derivatives)
    match cat {
        Cat::PublicDomain => (true, false, false, false),
        Cat::Permissive => (true, true, false, false),
        Cat::Attribution => (true, true, false, false),
        Cat::ShareAlike => (true, true, true, false),
        Cat::Copyleft => (true, true, true, false),
        Cat::NonCommercial => (false, true, false, false),
        Cat::NoDerivatives => (false, true, false, true),
        Cat::Proprietary => (false, false, false, false),
        Cat::Owner => (true, false, false, false),
    }
}

fn list(v: &Value, k: &str) -> Vec<String> {
    v.get(k).and_then(|a| a.as_arr()).map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_lowercase())).collect()).unwrap_or_default()
}

/// op "license_check": {license, hints?, policy?:{allow:[ids], deny:[ids], allow_noncommercial, allow_sharealike,
/// allow_copyleft, attested}} -> {id, status, reasons, commercial_ok, attribution, share_alike, no_derivatives}
pub fn check(v: &Value) -> Result<Value, String> {
    let lic = v.get("license").and_then(|x| x.as_str()).unwrap_or("");
    let mut candidates: Vec<String> = vec![lic.to_string()];
    if let Some(h) = v.get("hints").and_then(|h| h.as_arr()) {
        candidates.extend(h.iter().filter_map(|x| x.as_str().map(|s| s.to_string())));
    }
    let pol = v.get("policy").cloned().unwrap_or(Value::Null);
    let allow = list(&pol, "allow");
    let deny = list(&pol, "deny");
    let flag = |k: &str| pol.get(k).and_then(|x| x.as_bool()).unwrap_or(false);
    let mut found: Option<(String, Cat)> = None;
    for c in &candidates {
        if let Some(x) = classify(c) {
            found = Some(x);
            break;
        }
    }
    let mut reasons: Vec<String> = Vec::new();
    let (id, cat) = match found {
        Some(x) => x,
        None => {
            reasons.push(if lic.trim().is_empty() && candidates.len() == 1 { "no license information was provided".into() } else { "license text is not recognised".into() });
            reasons.push("without a clear license the content may not be used for training".into());
            let out = Value::obj(vec![
                ("id", Value::str("UNKNOWN")),
                ("status", Value::str("deny")),
                ("reasons", Value::Arr(reasons.into_iter().map(Value::str).collect())),
                ("commercial_ok", Value::Bool(false)),
                ("attribution", Value::Bool(false)),
                ("share_alike", Value::Bool(false)),
                ("no_derivatives", Value::Bool(false)),
            ]);
            return Ok(out);
        }
    };
    let (com, attr, sa, nd) = flags(cat);
    let idl = id.to_lowercase();
    let mut status = match cat {
        Cat::PublicDomain => "allow",
        Cat::Permissive | Cat::Attribution => "allow",
        Cat::ShareAlike => {
            reasons.push("share-alike: models/derivatives may have to carry the same license; confirm this fits your use".into());
            if flag("allow_sharealike") { "allow" } else { "review" }
        }
        Cat::Copyleft => {
            reasons.push("copyleft license: text/data reuse conditions are not settled; needs a decision".into());
            if flag("allow_copyleft") { "allow" } else { "review" }
        }
        Cat::NonCommercial => {
            reasons.push("non-commercial only: do not use if the dataset or model may be used commercially".into());
            if flag("allow_noncommercial") { "allow" } else { "review" }
        }
        Cat::NoDerivatives => {
            reasons.push("no-derivatives: training/transforming the content is not clearly permitted".into());
            "deny"
        }
        Cat::Proprietary => {
            reasons.push("all rights reserved: you need the owner's permission".into());
            "deny"
        }
        Cat::Owner => {
            reasons.push("you stated you own this data or have written permission".into());
            if flag("attested") { "allow" } else { "review" }
        }
    };
    if attr {
        reasons.push("attribution is required: the source and license are kept in the record metadata".into());
    }
    if deny.contains(&idl) {
        status = "deny";
        reasons.push("blocked by your policy".into());
    } else if allow.contains(&idl) && status != "deny" {
        status = "allow";
        reasons.push("allowed by your policy".into());
    }
    Ok(Value::obj(vec![
        ("id", Value::str(id)),
        ("status", Value::str(status)),
        ("reasons", Value::Arr(reasons.into_iter().map(Value::str).collect())),
        ("commercial_ok", Value::Bool(com)),
        ("attribution", Value::Bool(attr)),
        ("share_alike", Value::Bool(sa)),
        ("no_derivatives", Value::Bool(nd)),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::parse;

    fn go(j: &str) -> (String, String) {
        let r = check(&parse(j).unwrap()).unwrap();
        (r.get("id").unwrap().as_str().unwrap().to_string(), r.get("status").unwrap().as_str().unwrap().to_string())
    }

    #[test]
    fn classification() {
        assert_eq!(go(r#"{"license":"CC0 1.0"}"#), ("CC0-1.0".into(), "allow".into()));
        assert_eq!(go(r#"{"license":"https://creativecommons.org/licenses/by/4.0/"}"#), ("CC-BY-4.0".into(), "allow".into()));
        assert_eq!(go(r#"{"license":"CC BY-SA 3.0"}"#), ("CC-BY-SA-3.0".into(), "review".into()));
        assert_eq!(go(r#"{"license":"cc-by-nc-4.0"}"#), ("CC-BY-NC-4.0".into(), "review".into()));
        assert_eq!(go(r#"{"license":"CC BY-NC-ND 4.0"}"#), ("CC-BY-NC-ND-4.0".into(), "deny".into()));
        assert_eq!(go(r#"{"license":"MIT"}"#), ("MIT".into(), "allow".into()));
        assert_eq!(go(r#"{"license":"Apache License 2.0"}"#), ("Apache-2.0".into(), "allow".into()));
        assert_eq!(go(r#"{"license":"GPL-3.0"}"#), ("GPL-3.0".into(), "review".into()));
        assert_eq!(go(r#"{"license":"All rights reserved"}"#), ("All-Rights-Reserved".into(), "deny".into()));
        assert_eq!(go(r#"{"license":""}"#), ("UNKNOWN".into(), "deny".into()));
        assert_eq!(go(r#"{"license":"whatever"}"#), ("UNKNOWN".into(), "deny".into()));
        assert_eq!(go(r#"{"license":"","hints":["https://creativecommons.org/licenses/by/4.0/"]}"#).1, "allow");
        assert_eq!(go(r#"{"license":"CC-BY-NC-4.0","policy":{"allow_noncommercial":true}}"#).1, "allow");
        assert_eq!(go(r#"{"license":"MIT","policy":{"deny":["MIT"]}}"#).1, "deny");
        assert_eq!(go(r#"{"license":"I own this data"}"#).1, "review");
        assert_eq!(go(r#"{"license":"I own this data","policy":{"attested":true}}"#).1, "allow");
    }
}
