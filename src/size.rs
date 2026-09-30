//! 命令行里的大小（500M）和时长（90d）参数解析。

use std::time::Duration;

/// 解析 `500M`、`2G`、`1024` 这类大小，按 1024 进制，单位不区分大小写。
pub fn parse_size(s: &str) -> Result<u64, String> {
    let s = s.trim();
    let split = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    let num: u64 = num.parse().map_err(|_| format!("无效的大小: {s}"))?;
    let mul: u64 = match unit.to_ascii_uppercase().as_str() {
        "" | "B" => 1,
        "K" | "KB" => 1 << 10,
        "M" | "MB" => 1 << 20,
        "G" | "GB" => 1 << 30,
        "T" | "TB" => 1 << 40,
        _ => return Err(format!("无效的大小单位: {s}（可用 B/K/M/G/T）")),
    };
    num.checked_mul(mul).ok_or_else(|| format!("大小溢出: {s}"))
}

/// 解析 `12h`、`90d`、`4w` 这类时长。
pub fn parse_duration(s: &str) -> Result<Duration, String> {
    let s = s.trim();
    let split = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    let num: u64 = num.parse().map_err(|_| format!("无效的时长: {s}"))?;
    let secs: u64 = match unit {
        "h" => 3600,
        "d" => 86_400,
        "w" => 7 * 86_400,
        _ => return Err(format!("无效的时长单位: {s}（可用 h/d/w）")),
    };
    num.checked_mul(secs)
        .map(Duration::from_secs)
        .ok_or_else(|| format!("时长溢出: {s}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(parse_size("1024"), Ok(1024));
        assert_eq!(parse_size("500M"), Ok(500 << 20));
        assert_eq!(parse_size("2g"), Ok(2 << 30));
        assert_eq!(parse_size("4KB"), Ok(4096));
        assert!(parse_size("M").is_err());
        assert!(parse_size("5X").is_err());
        assert!(parse_size("-1").is_err());
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration("90d"), Ok(Duration::from_secs(90 * 86_400)));
        assert_eq!(parse_duration("2w"), Ok(Duration::from_secs(14 * 86_400)));
        assert_eq!(parse_duration("12h"), Ok(Duration::from_secs(12 * 3600)));
        assert!(parse_duration("90").is_err());
        assert!(parse_duration("3m").is_err());
    }
}
