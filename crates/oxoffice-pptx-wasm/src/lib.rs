//! WebAssembly bindings for rpptx.

use wasm_bindgen::prelude::*;

/// A PowerPoint-compatible presentation that can be opened, edited, and saved.
#[wasm_bindgen]
pub struct WasmPresentation {
    inner: oxoffice_pptx::Presentation,
}

#[wasm_bindgen]
impl WasmPresentation {
    /// Creates a new presentation from the bundled default template.
    #[wasm_bindgen(constructor)]
    pub fn new() -> Result<WasmPresentation, JsValue> {
        let inner = oxoffice_pptx::Presentation::new().map_err(to_js_error)?;
        Ok(Self { inner })
    }

    /// Opens a presentation from PPTX bytes.
    #[wasm_bindgen(js_name = "fromBytes")]
    pub fn from_bytes(data: &[u8]) -> Result<WasmPresentation, JsValue> {
        let inner = oxoffice_pptx::Presentation::from_bytes(data).map_err(to_js_error)?;
        Ok(Self { inner })
    }

    /// Serializes the complete presentation package.
    #[wasm_bindgen(js_name = "toBytes")]
    pub fn to_bytes(&self) -> Result<Vec<u8>, JsValue> {
        self.inner.to_bytes().map_err(to_js_error)
    }

    /// Returns the number of slides.
    #[wasm_bindgen(js_name = "slideCount")]
    pub fn slide_count(&self) -> u32 {
        self.inner.len() as u32
    }

    /// Adds a slide from the zero-based layout index.
    #[wasm_bindgen(js_name = "addSlide")]
    pub fn add_slide(&mut self, layout_index: u32) -> Result<(), JsValue> {
        self.inner
            .add_slide(layout_index as usize)
            .map(|_| ())
            .map_err(to_js_error)
    }

    /// Renders the presentation to a deterministic PDF.
    #[cfg(feature = "render")]
    #[wasm_bindgen(js_name = "toPdf")]
    pub fn to_pdf(&self) -> Result<Vec<u8>, JsValue> {
        self.inner.to_pdf_deterministic().map_err(to_js_error)
    }
}

fn to_js_error(error: oxoffice_pptx::Error) -> JsValue {
    JsValue::from_str(&error.to_string())
}
