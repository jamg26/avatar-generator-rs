use anyhow::{Context, Result};
use image::DynamicImage;
use reqwest::Client;
use std::time::Duration;

use crate::generator::prompt::{
    AvatarRequest, Accessory, Background, Ethnicity, Expression,
    FacialHair, HairColor, HairStyle, Sex, SkinTone,
};

const DICEBEAR_BASE: &str = "https://api.dicebear.com/9.x/avataaars/png";

/// Generates cartoon-style avatar PNGs via the DiceBear avataaars CDN.
/// No model loading — always ready; typical latency <1 s.
#[derive(Clone)]
pub struct DicebearPipeline {
    client: Client,
}

impl DicebearPipeline {
    pub fn new() -> Result<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent("avagen/0.1")
            .build()
            .context("Failed to build HTTP client for DiceBear")?;
        tracing::info!("DiceBear avatar pipeline ready (no model loading required)");
        Ok(Self { client })
    }

    /// Calls the DiceBear API and returns the decoded PNG as a `DynamicImage`.
    pub async fn generate(
        &self,
        req: &AvatarRequest,
        size: usize,
        seed: u64,
    ) -> Result<DynamicImage> {
        // Cap at 512 — DiceBear freely supports this size
        let px = size.min(512).to_string();

        let mut params: Vec<(&'static str, String)> = vec![
            ("seed",  seed.to_string()),
            ("size",  px),
            ("radius", "0".to_string()),
        ];

        // ── Skin ─────────────────────────────────────────────────────────────
        params.push(("skinColor[]", map_skin(&req.ethnicity, req.skin_tone.as_ref()).into()));

        // ── Hair / top ────────────────────────────────────────────────────────
        // Hijab/turban/hat accessories override the hair style (they ARE the top).
        let top = if req.accessories.contains(&Accessory::Hijab) {
            "hijab"
        } else if req.accessories.contains(&Accessory::Turban) {
            "turban"
        } else if req.accessories.contains(&Accessory::Hat) {
            "hat"
        } else {
            map_top(&req.hair_style)
        };
        params.push(("top[]",       top.into()));
        params.push(("hairColor[]", map_hair_color(&req.hair_color).into()));

        // ── Facial hair ───────────────────────────────────────────────────────
        if matches!(req.facial_hair, FacialHair::None) {
            params.push(("facialHairProbability", "0".into()));
        } else {
            params.push(("facialHair[]",          map_facial_hair(&req.facial_hair).into()));
            // Match facial-hair colour to head-hair colour (same hex values)
            params.push(("facialHairColor[]",     map_hair_color(&req.hair_color).into()));
            params.push(("facialHairProbability", "100".into()));
        }

        // ── Eyes & mouth (driven by expression) ──────────────────────────────
        params.push(("eyes[]",  map_eyes(&req.expression).into()));
        params.push(("mouth[]", map_mouth(&req.expression).into()));

        // ── Glasses / sunglasses ──────────────────────────────────────────────
        let eyewear = req.accessories.iter().find_map(|a| match a {
            Accessory::Glasses    => Some("prescription01"),
            Accessory::Sunglasses => Some("sunglasses"),
            _ => None,
        });
        if let Some(ew) = eyewear {
            params.push(("accessories[]",          ew.into()));
            params.push(("accessoriesProbability", "100".into()));
        } else {
            params.push(("accessoriesProbability", "0".into()));
        }

        // ── Clothing (sex-appropriate) ────────────────────────────────────────
        params.push(("clothing[]", map_clothing(&req.sex).into()));

        // ── Background ────────────────────────────────────────────────────────
        let (bg_color, bg_type) = map_background(&req.background);
        params.push(("backgroundColor[]", bg_color.into()));
        if let Some(bt) = bg_type {
            params.push(("backgroundType[]", bt.into()));
        }

        tracing::debug!("DiceBear GET {} {:?}", DICEBEAR_BASE, params);

        let response = self
            .client
            .get(DICEBEAR_BASE)
            .query(&params)
            .send()
            .await
            .context("DiceBear API request failed")?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("DiceBear API returned {}: {}", status, body);
        }

        let bytes = response.bytes().await.context("Failed to read DiceBear response")?;

        let img = tokio::task::spawn_blocking(move || image::load_from_memory(&bytes))
            .await
            .map_err(|e| anyhow::anyhow!("image decode join: {e}"))?
            .context("Failed to decode PNG from DiceBear")?;

        tracing::debug!("DiceBear returned {}×{} image", img.width(), img.height());
        Ok(img)
    }
}

// ── Trait → DiceBear parameter mappings ─────────────────────────────────────

fn map_skin(ethnicity: &Ethnicity, tone: Option<&SkinTone>) -> &'static str {
    // DiceBear v9 requires 6-digit hex for skinColor (no '#')
    if let Some(t) = tone {
        return match t {
            SkinTone::VeryLight   => "ffdbb4",
            SkinTone::Light       => "edb98a",
            SkinTone::MediumLight => "d08b5b",
            SkinTone::Medium      => "f8d25c",
            SkinTone::MediumDark  => "ae5d29",
            SkinTone::Dark        => "614335",
            SkinTone::VeryDark    => "4a312c",
        };
    }
    match ethnicity {
        Ethnicity::Caucasian       => "edb98a",
        Ethnicity::African         => "614335",
        Ethnicity::EastAsian       => "f8d25c",
        Ethnicity::SouthAsian      => "ae5d29",
        Ethnicity::SoutheastAsian  => "d08b5b",
        Ethnicity::Hispanic        => "d08b5b",
        Ethnicity::MiddleEastern   => "ae5d29",
        Ethnicity::NativeAmerican  => "ae5d29",
        Ethnicity::PacificIslander => "d08b5b",
        Ethnicity::Mixed           => "d08b5b",
    }
}

/// Maps our HairStyle enum to DiceBear avataaars v9 `top` values.
fn map_top(style: &HairStyle) -> &'static str {
    match style {
        HairStyle::Bald         => "shavedSides",
        HairStyle::BuzzCut      => "theCaesar",
        HairStyle::Short        => "shortRound",
        HairStyle::Medium       => "longButNotTooLong",
        HairStyle::LongStraight => "straight01",
        HairStyle::LongWavy     => "curvy",
        HairStyle::LongCurly    => "curly",
        HairStyle::Afro         => "fro",
        HairStyle::Braids       => "dreads01",
        HairStyle::Ponytail     => "miaWallace",
        HairStyle::Bun          => "bun",
        HairStyle::Mohawk       => "sides",
        HairStyle::Dreadlocks   => "dreads02",
    }
}

fn map_hair_color(color: &HairColor) -> &'static str {
    // DiceBear avataaars v9 expects 6-digit hex for hairColor (no '#')
    match color {
        HairColor::Black            => "2c1b18",
        HairColor::Brown            => "724133",
        HairColor::Blonde           => "b58143",
        HairColor::Red              => "c93305",
        HairColor::Gray             => "e8e1ef",
        HairColor::White            => "ecdcbf",
        HairColor::Auburn           => "a55728",
        HairColor::StrawberryBlonde => "f59797",
    }
}

fn map_facial_hair(fh: &FacialHair) -> &'static str {
    match fh {
        FacialHair::None      => "beardLight", // probability=0 guards this branch
        FacialHair::Stubble   => "beardLight",
        FacialHair::Mustache  => "moustacheFancy",
        FacialHair::Goatee    => "beardLight",
        FacialHair::FullBeard => "beardMedium",
        FacialHair::LongBeard => "beardMajestic",
    }
}

fn map_eyes(expr: &Expression) -> &'static str {
    match expr {
        Expression::Neutral    => "default",
        Expression::Happy      => "happy",
        Expression::Serious    => "squint",
        Expression::Confident  => "default",
        Expression::Friendly   => "happy",
        Expression::Thoughtful => "side",
        Expression::Surprised  => "surprised",
    }
}

fn map_mouth(expr: &Expression) -> &'static str {
    match expr {
        Expression::Neutral    => "serious",
        Expression::Happy      => "smile",
        Expression::Serious    => "default",
        Expression::Confident  => "twinkle",
        Expression::Friendly   => "tongue",
        Expression::Thoughtful => "concerned",
        Expression::Surprised  => "screamOpen",
    }
}

fn map_clothing(sex: &Sex) -> &'static str {
    match sex {
        Sex::Male   => "shirtCrewNeck",
        Sex::Female => "shirtScoopNeck",
    }
}

/// Returns (backgroundColor hex, optional backgroundType) for DiceBear.
fn map_background(bg: &Background) -> (&'static str, Option<&'static str>) {
    match bg {
        Background::White    => ("ffffff",   None),
        Background::Gray     => ("b0b4ba",   None),
        Background::Blue     => ("648cc8",   None),
        Background::Gradient => ("d2b9eb",   Some("gradientLinear")),
        Background::Nature   => ("78aa6e",   None),
        Background::Studio   => ("323746",   None),
    }
}
