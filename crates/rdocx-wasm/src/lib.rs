//! WebAssembly bindings for rdocx.
//!
//! Provides JavaScript-friendly API for creating, opening, and converting
//! DOCX documents entirely in the browser or Node.js — no server needed.

use wasm_bindgen::prelude::*;

/// A Word document (.docx) that can be created, modified, and exported.
#[wasm_bindgen]
pub struct WasmDocument {
    inner: rdocx::Document,
}

impl Default for WasmDocument {
    fn default() -> Self {
        Self::new()
    }
}

#[wasm_bindgen]
impl WasmDocument {
    /// Create a new, empty document.
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self {
            inner: rdocx::Document::new(),
        }
    }

    /// Open a document from DOCX bytes.
    #[wasm_bindgen(js_name = "fromBytes")]
    pub fn from_bytes(data: &[u8]) -> Result<WasmDocument, JsValue> {
        let inner = rdocx::Document::from_bytes(data).map_err(to_js_error)?;
        Ok(Self { inner })
    }

    /// Add a paragraph with the given text.
    #[wasm_bindgen(js_name = "addParagraph")]
    pub fn add_paragraph(&mut self, text: &str) {
        self.inner.add_paragraph(text);
    }

    /// Add a heading paragraph (level 1-6).
    #[wasm_bindgen(js_name = "addHeading")]
    pub fn add_heading(&mut self, text: &str, level: u32) {
        let level = level.clamp(1, 6);
        self.inner
            .add_paragraph(text)
            .set_style(&format!("Heading{level}"));
    }

    /// Add a paragraph with bold text.
    #[wasm_bindgen(js_name = "addBoldParagraph")]
    pub fn add_bold_paragraph(&mut self, text: &str) {
        self.inner.add_paragraph("").add_run(text).set_bold(true);
    }

    /// Add a simple table with the given number of rows and columns.
    #[wasm_bindgen(js_name = "addTable")]
    pub fn add_table(&mut self, rows: u32, cols: u32) {
        self.inner.add_table(rows as usize, cols as usize);
    }

    /// Get the text content of the entire document.
    #[wasm_bindgen(js_name = "getText")]
    pub fn get_text(&self) -> String {
        self.inner.text()
    }

    /// Get the number of paragraphs in the document.
    #[wasm_bindgen(js_name = "paragraphCount")]
    pub fn paragraph_count(&self) -> u32 {
        self.inner.paragraph_count() as u32
    }

    /// Export as DOCX bytes.
    #[wasm_bindgen(js_name = "toDocxBytes")]
    pub fn to_docx_bytes(&mut self) -> Result<Vec<u8>, JsValue> {
        self.inner.to_bytes().map_err(to_js_error)
    }

    /// Render the document as PDF bytes.
    #[wasm_bindgen(js_name = "toPdf")]
    pub fn to_pdf(&self) -> Result<Vec<u8>, JsValue> {
        self.inner.to_pdf().map_err(to_js_error)
    }

    /// Convert to a complete HTML document string.
    #[wasm_bindgen(js_name = "toHtml")]
    pub fn to_html(&self) -> String {
        self.inner.to_html()
    }

    /// Convert to an HTML fragment (body content only).
    #[wasm_bindgen(js_name = "toHtmlFragment")]
    pub fn to_html_fragment(&self) -> String {
        self.inner.to_html_fragment()
    }

    /// Convert to Markdown.
    #[wasm_bindgen(js_name = "toMarkdown")]
    pub fn to_markdown(&self) -> String {
        self.inner.to_markdown()
    }

    /// Replace all occurrences of a placeholder with a value.
    #[wasm_bindgen(js_name = "replacePlaceholder")]
    pub fn replace_placeholder(&mut self, placeholder: &str, value: &str) -> u32 {
        self.inner.replace_text(placeholder, value) as u32
    }
}

fn to_js_error(error: rdocx::Error) -> JsValue {
    JsValue::from_str(&error.to_string())
}
