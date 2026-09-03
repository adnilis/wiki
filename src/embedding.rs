//! Pluggable embedding providers used by hybrid RAG retrieval.
//!
//! The built-in provider is intentionally local and deterministic. It uses
//! feature hashing over tokens and character n-grams, so it exercises the
//! same provider/cache interface that a future model-backed provider can use
//! without introducing network or runtime model dependencies.

use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub const HASH_EMBEDDING_DIMENSIONS: usize = 384;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum EmbeddingMode {
    #[default]
    None,
    Hash,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EmbeddingDescriptor {
    pub provider: String,
    pub model: String,
    pub dimensions: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EmbeddingError(pub String);

impl std::fmt::Display for EmbeddingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Provider boundary for local or remote embedding implementations.
///
/// Implementations must return finite vectors with the advertised dimension.
/// `embed_batch` can be overridden by providers with a native batch API.
pub trait EmbeddingProvider {
    fn descriptor(&self) -> EmbeddingDescriptor;

    fn embed(&self, input: &str) -> Result<Vec<f32>, EmbeddingError>;

    fn embed_batch(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>, EmbeddingError> {
        inputs.iter().map(|input| self.embed(input)).collect()
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct HashEmbeddingProvider;

impl EmbeddingProvider for HashEmbeddingProvider {
    fn descriptor(&self) -> EmbeddingDescriptor {
        EmbeddingDescriptor {
            provider: "local-hash".to_string(),
            model: "token-ngram-v1".to_string(),
            dimensions: HASH_EMBEDDING_DIMENSIONS,
        }
    }

    fn embed(&self, input: &str) -> Result<Vec<f32>, EmbeddingError> {
        let descriptor = self.descriptor();
        let mut feature_counts: BTreeMap<String, usize> = BTreeMap::new();
        for token in tokenize(input) {
            *feature_counts.entry(format!("token:{token}")).or_default() += 1;
            let characters: Vec<char> = token.chars().collect();
            for width in [2usize, 3usize] {
                if characters.len() < width {
                    continue;
                }
                for window in characters.windows(width) {
                    let ngram: String = window.iter().collect();
                    *feature_counts
                        .entry(format!("ngram:{width}:{ngram}"))
                        .or_default() += 1;
                }
            }
        }

        let mut vector = vec![0.0f32; descriptor.dimensions];
        for (feature, count) in feature_counts {
            let digest = Sha256::digest(feature.as_bytes());
            let index = u64::from_le_bytes(digest[0..8].try_into().expect("fixed digest slice"))
                as usize
                % descriptor.dimensions;
            let sign = if digest[8] & 1 == 0 { 1.0 } else { -1.0 };
            let family_weight = if feature.starts_with("token:") {
                1.0
            } else {
                0.35
            };
            let term_frequency = 1.0 + (count as f32).ln();
            vector[index] += sign * family_weight * term_frequency;
        }
        normalize(&mut vector);
        Ok(vector)
    }
}

pub fn provider(mode: EmbeddingMode) -> Option<Box<dyn EmbeddingProvider>> {
    match mode {
        EmbeddingMode::None => None,
        EmbeddingMode::Hash => Some(Box::new(HashEmbeddingProvider)),
    }
}

pub fn cosine_similarity(left: &[f32], right: &[f32]) -> Option<f64> {
    if left.is_empty() || left.len() != right.len() {
        return None;
    }
    let mut dot = 0.0f64;
    let mut left_norm = 0.0f64;
    let mut right_norm = 0.0f64;
    for (&left_value, &right_value) in left.iter().zip(right) {
        if !left_value.is_finite() || !right_value.is_finite() {
            return None;
        }
        let left_value = left_value as f64;
        let right_value = right_value as f64;
        dot += left_value * right_value;
        left_norm += left_value * left_value;
        right_norm += right_value * right_value;
    }
    if left_norm == 0.0 || right_norm == 0.0 {
        return Some(0.0);
    }
    Some((dot / (left_norm.sqrt() * right_norm.sqrt())).clamp(-1.0, 1.0))
}

fn tokenize(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut previous_was_lower_or_digit = false;

    for character in input.chars() {
        let accepted = character.is_alphanumeric() || matches!(character, '_' | '-');
        if !accepted {
            push_token(&mut tokens, &mut current);
            previous_was_lower_or_digit = false;
            continue;
        }
        if character.is_uppercase() && previous_was_lower_or_digit && !current.is_empty() {
            push_token(&mut tokens, &mut current);
        }
        previous_was_lower_or_digit = character.is_lowercase() || character.is_numeric();
        current.extend(character.to_lowercase());
    }
    push_token(&mut tokens, &mut current);
    tokens
}

fn push_token(tokens: &mut Vec<String>, current: &mut String) {
    if !current.is_empty() {
        tokens.push(std::mem::take(current));
    }
}

fn normalize(vector: &mut [f32]) {
    let norm = vector
        .iter()
        .map(|value| (*value as f64) * (*value as f64))
        .sum::<f64>()
        .sqrt();
    if norm == 0.0 {
        return;
    }
    for value in vector {
        *value = (*value as f64 / norm) as f32;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_embeddings_are_stable_and_normalized() {
        let provider = HashEmbeddingProvider;
        let first = provider.embed("RoleService handles permissions").unwrap();
        let second = provider.embed("RoleService handles permissions").unwrap();
        assert_eq!(first, second);
        assert_eq!(first.len(), HASH_EMBEDDING_DIMENSIONS);
        let norm = first
            .iter()
            .map(|value| (*value as f64) * (*value as f64))
            .sum::<f64>()
            .sqrt();
        assert!((norm - 1.0).abs() < 1e-5);
    }

    #[test]
    fn ngrams_rank_related_words_above_unrelated_words() {
        let provider = HashEmbeddingProvider;
        let query = provider.embed("permissions").unwrap();
        let related = provider.embed("permission checks").unwrap();
        let unrelated = provider.embed("database migration").unwrap();
        assert!(
            cosine_similarity(&query, &related).unwrap()
                > cosine_similarity(&query, &unrelated).unwrap()
        );
    }
}
