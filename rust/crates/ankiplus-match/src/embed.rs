//! Local ONNX embeddings through fastembed-rs (no API calls).
//!
//! Same model and post-processing as the Python `FastEmbedder`, so vectors stored
//! by either side are interchangeable: `nomic-embed-text-v1.5` with the
//! `search_document: ` / `search_query: ` task prefixes, then Matryoshka truncation
//! (layer norm, cut to `dim`, re-normalise).

use crate::index::{normalise, Embedder};
use crate::Error;
use fastembed::{EmbeddingModel, TextEmbedding, TextInitOptions};
use std::sync::Mutex;

pub const DEFAULT_MODEL: &str = "nomic-ai/nomic-embed-text-v1.5";

pub struct FastEmbedder {
    model_name: String,
    dim: Option<usize>,
    doc_prefix: &'static str,
    query_prefix: &'static str,
    model: Mutex<TextEmbedding>,
}

impl FastEmbedder {
    /// Only the Nomic v1.5 model is wired up for now; it is the one the Python index uses.
    pub fn new(cache_dir: Option<std::path::PathBuf>, dim: usize) -> Result<Self, Error> {
        let mut opts = TextInitOptions::new(EmbeddingModel::NomicEmbedTextV15);
        if let Some(dir) = cache_dir {
            opts = opts.with_cache_dir(dir);
        }
        let model = TextEmbedding::try_new(opts).map_err(|e| Error::Embed(e.to_string()))?;
        Ok(Self {
            model_name: DEFAULT_MODEL.into(),
            dim: Some(dim),
            doc_prefix: "search_document: ",
            query_prefix: "search_query: ",
            model: Mutex::new(model),
        })
    }

    fn encode(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>, Error> {
        let raw = self
            .model
            .lock()
            .map_err(|_| Error::Embed("embedding model lock poisoned".into()))?
            .embed(&texts, None)
            .map_err(|e| Error::Embed(e.to_string()))?;
        Ok(raw.into_iter().map(|v| self.post(v)).collect())
    }

    /// Layer norm (population std, eps 1e-5), truncate, L2-normalise.
    pub fn post(&self, mut v: Vec<f32>) -> Vec<f32> {
        if let Some(dim) = self.dim {
            let n = v.len() as f32;
            let mean = v.iter().sum::<f32>() / n;
            let std = (v.iter().map(|x| (x - mean).powi(2)).sum::<f32>() / n).sqrt();
            v.iter_mut().for_each(|x| *x = (*x - mean) / (std + 1e-5));
            v.truncate(dim);
        }
        normalise(&mut v);
        v
    }
}

impl Embedder for FastEmbedder {
    fn name(&self) -> String {
        match self.dim {
            Some(d) => format!("{}@{d}", self.model_name),
            None => self.model_name.clone(),
        }
    }

    fn embed_documents(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, Error> {
        self.encode(texts.iter().map(|t| format!("{}{t}", self.doc_prefix)).collect())
    }

    fn embed_query(&self, text: &str) -> Result<Vec<f32>, Error> {
        Ok(self.encode(vec![format!("{}{text}", self.query_prefix)])?.remove(0))
    }
}
