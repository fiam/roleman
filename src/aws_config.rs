use crate::model::RoleChoice;

pub fn profile_name_for(choice: &RoleChoice) -> String {
    let raw = format!("roleman-{}-{}", choice.account_id, choice.role_name);
    sanitize_profile_name(&raw)
}

fn sanitize_profile_name(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            out.push(ch);
        } else {
            out.push('-');
        }
    }
    while out.contains("--") {
        out = out.replace("--", "-");
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "roleman".to_string()
    } else {
        trimmed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_profile_name() {
        assert_eq!(
            sanitize_profile_name("roleman-1234-Admin Role"),
            "roleman-1234-Admin-Role"
        );
    }
}
