//! Argon2id 密码哈希与弱密码策略。

use argon2::password_hash::{PasswordHash, PasswordHasher as _, PasswordVerifier, SaltString};
use argon2::{Algorithm, Argon2, Params, Version};
use password_hash::rand_core::OsRng;
use thiserror::Error;

/// 弱密码策略错误。
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PasswordPolicyError {
    #[error("密码长度至少 {min} 个字符")]
    TooShort { min: usize },
    #[error("密码长度不得超过 {max} 个字符")]
    TooLong { max: usize },
    #[error("密码必须同时包含字母与数字")]
    MissingLetterOrDigit,
    #[error("密码过于常见，请更换")]
    CommonPassword,
}

/// 密码强度策略（可配置）。
#[derive(Debug, Clone)]
pub struct PasswordPolicy {
    pub min_length: usize,
    pub max_length: usize,
    pub require_letter_and_digit: bool,
}

impl Default for PasswordPolicy {
    fn default() -> Self {
        Self {
            min_length: 10,
            max_length: 128,
            require_letter_and_digit: true,
        }
    }
}

impl PasswordPolicy {
    /// 校验明文密码强度；不记录明文。
    pub fn validate(&self, password: &str) -> Result<(), PasswordPolicyError> {
        let len = password.chars().count();
        if len < self.min_length {
            return Err(PasswordPolicyError::TooShort {
                min: self.min_length,
            });
        }
        if len > self.max_length {
            return Err(PasswordPolicyError::TooLong {
                max: self.max_length,
            });
        }
        if self.require_letter_and_digit {
            let has_letter = password.chars().any(|c| c.is_ascii_alphabetic());
            let has_digit = password.chars().any(|c| c.is_ascii_digit());
            if !(has_letter && has_digit) {
                return Err(PasswordPolicyError::MissingLetterOrDigit);
            }
        }
        if is_common_password(password) {
            return Err(PasswordPolicyError::CommonPassword);
        }
        Ok(())
    }
}

fn is_common_password(password: &str) -> bool {
    const COMMON: &[&str] = &[
        "password",
        "password1",
        "password123",
        "1234567890",
        "qwerty1234",
        "admin12345",
        "letmein123",
        "welcome123",
        "changeme12",
        "signal1234",
    ];
    let lower = password.to_ascii_lowercase();
    COMMON.iter().any(|c| *c == lower)
}

/// Argon2id 哈希器；参数集中配置。
#[derive(Debug, Clone)]
pub struct PasswordHasher {
    argon2: Argon2<'static>,
    policy: PasswordPolicy,
}

impl Default for PasswordHasher {
    fn default() -> Self {
        Self::new(PasswordPolicy::default())
    }
}

impl PasswordHasher {
    /// 使用 OWASP 推荐量级的 Argon2id 参数（可后续迁移）。
    pub fn new(policy: PasswordPolicy) -> Self {
        let params = Params::new(19_456, 2, 1, None).expect("valid argon2 params");
        let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
        Self { argon2, policy }
    }

    pub fn policy(&self) -> &PasswordPolicy {
        &self.policy
    }

    /// 校验策略后哈希；失败不泄露明文。
    pub fn hash_password(&self, password: &str) -> Result<String, PasswordPolicyError> {
        self.policy.validate(password)?;
        let salt = SaltString::generate(&mut OsRng);
        let hash = self
            .argon2
            .hash_password(password.as_bytes(), &salt)
            .expect("argon2 hash")
            .to_string();
        Ok(hash)
    }

    /// 校验明文与已存 PHC 字符串；恒定时间比较由库保证。
    pub fn verify_password(&self, password: &str, password_hash: &str) -> bool {
        let Ok(parsed) = PasswordHash::new(password_hash) else {
            return false;
        };
        self.argon2
            .verify_password(password.as_bytes(), &parsed)
            .is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_short_password() {
        let h = PasswordHasher::default();
        let err = h.hash_password("Ab1").unwrap_err();
        assert!(matches!(err, PasswordPolicyError::TooShort { .. }));
    }

    #[test]
    fn rejects_missing_digit() {
        let h = PasswordHasher::default();
        let err = h.hash_password("abcdefghij").unwrap_err();
        assert_eq!(err, PasswordPolicyError::MissingLetterOrDigit);
    }

    #[test]
    fn rejects_common_password() {
        let h = PasswordHasher::default();
        let err = h.hash_password("password123").unwrap_err();
        assert_eq!(err, PasswordPolicyError::CommonPassword);
    }

    #[test]
    fn hash_is_not_plaintext_and_verifies() {
        let h = PasswordHasher::default();
        let password = "CorrectHorse9!";
        let hash = h.hash_password(password).expect("hash");
        assert!(!hash.contains(password));
        assert!(hash.starts_with("$argon2id$"));
        assert!(h.verify_password(password, &hash));
        assert!(!h.verify_password("WrongHorse9!", &hash));
    }

    #[test]
    fn same_password_produces_different_salts() {
        let h = PasswordHasher::default();
        let a = h.hash_password("UniquePass1!").unwrap();
        let b = h.hash_password("UniquePass1!").unwrap();
        assert_ne!(a, b);
        assert!(h.verify_password("UniquePass1!", &a));
        assert!(h.verify_password("UniquePass1!", &b));
    }
}
