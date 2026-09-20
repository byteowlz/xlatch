//! Portable embedded icons. SVG authoring is rasterized once, before registration.
use anyhow::{Context, Result, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use image::{ImageFormat, ImageReader, Limits};
use serde::{Deserialize, Serialize};
use std::{
    io::{Cursor, Read as _},
    path::Path,
};

/// A bounded PNG travels with the approved manifest and offline target cache.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Icon {
    /// Base64 PNG, at most 128 `KiB` and 256 pixels on either axis.
    pub png_base64: String,
}
impl Icon {
    /// Decode and validate untrusted icon bytes with allocation and dimension limits.
    /// # Errors
    /// Rejects malformed or oversized images.
    pub fn png_bytes(&self) -> Result<Vec<u8>> {
        ensure!(self.png_base64.len() <= 175_000, "icon exceeds 128 KiB");
        let bytes = STANDARD.decode(&self.png_base64)?;
        ensure!(bytes.len() <= 128 * 1024, "icon exceeds 128 KiB");
        decode_png(&bytes, 256)?;
        Ok(bytes)
    }
    /// Convert a local SVG or PNG source into a bounded native-client image.
    /// # Errors
    /// Rejects unsupported SVG features, malformed images and excessive source sizes.
    pub fn from_file(path: &Path) -> Result<Self> {
        let mut bytes = Vec::new();
        std::fs::File::open(path)?
            .take(1024 * 1024 + 1)
            .read_to_end(&mut bytes)?;
        ensure!(bytes.len() <= 1024 * 1024, "icon source exceeds 1 MiB");
        let png = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            let image = decode_png(&bytes, 4096)?.thumbnail(128, 128);
            let mut output = Cursor::new(Vec::new());
            image.write_to(&mut output, ImageFormat::Png)?;
            output.into_inner()
        } else {
            rasterize_svg(&bytes)?
        };
        let icon = Self {
            png_base64: STANDARD.encode(png),
        };
        icon.png_bytes()?;
        Ok(icon)
    }
}
fn decode_png(bytes: &[u8], side: u32) -> Result<image::DynamicImage> {
    let mut reader = ImageReader::with_format(Cursor::new(bytes), ImageFormat::Png);
    let mut limits = Limits::default();
    limits.max_image_width = Some(side);
    limits.max_image_height = Some(side);
    limits.max_alloc = Some(80 * 1024 * 1024);
    reader.limits(limits);
    Ok(reader.decode()?)
}
fn rasterize_svg(bytes: &[u8]) -> Result<Vec<u8>> {
    ensure!(bytes.len() <= 64 * 1024, "SVG icon exceeds 64 KiB");
    let text = std::str::from_utf8(bytes)?;
    let document = resvg::usvg::roxmltree::Document::parse(text)?;
    ensure!(
        document.descendants().count() <= 1024,
        "SVG icon is too complex"
    );
    for node in document
        .descendants()
        .filter(resvg::usvg::roxmltree::Node::is_element)
    {
        ensure!(
            [
                "svg",
                "g",
                "path",
                "rect",
                "circle",
                "ellipse",
                "line",
                "polyline",
                "polygon",
                "defs",
                "linearGradient",
                "radialGradient",
                "stop",
                "clipPath",
                "title",
                "desc"
            ]
            .contains(&node.tag_name().name()),
            "unsupported SVG element {}; use outlined shapes without text, filters, images or scripts",
            node.tag_name().name()
        );
        for attr in node.attributes() {
            ensure!(
                !attr.name().starts_with("on"),
                "SVG event handlers are not supported"
            );
            if attr.name() == "href" {
                ensure!(
                    attr.value().starts_with('#'),
                    "SVG external references are not supported"
                );
            }
        }
    }
    let options = resvg::usvg::Options {
        image_href_resolver: resvg::usvg::ImageHrefResolver {
            resolve_data: Box::new(|_, _, _| None),
            resolve_string: Box::new(|_, _| None),
        },
        ..resvg::usvg::Options::default()
    };
    let tree = resvg::usvg::Tree::from_str(text, &options)?;
    let scale = 128.0 / tree.size().width().max(tree.size().height());
    let x = tree.size().width().mul_add(-scale, 128.0) / 2.0;
    let y = tree.size().height().mul_add(-scale, 128.0) / 2.0;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(128, 128).context("allocate icon")?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_row(scale, 0.0, 0.0, scale, x, y),
        &mut pixmap.as_mut(),
    );
    Ok(pixmap.encode_png()?)
}
