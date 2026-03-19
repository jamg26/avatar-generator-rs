use serde::{Deserialize, Serialize};

// ── Request types ────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Serialize)]
pub struct AvatarRequest {
    /// baby | toddler | child | teenager | young_adult | adult | middle_aged | senior | elderly
    pub age: Age,
    /// male | female
    pub sex: Sex,
    /// caucasian | african | east_asian | south_asian | southeast_asian |
    /// hispanic | middle_eastern | native_american | pacific_islander | mixed
    pub ethnicity: Ethnicity,
    /// black | brown | blonde | red | gray | white | auburn | strawberry_blonde
    #[serde(default = "default_hair_color")]
    pub hair_color: HairColor,
    /// bald | buzz_cut | short | medium | long_straight | long_wavy |
    /// long_curly | afro | braids | ponytail | bun | mohawk | dreadlocks
    #[serde(default = "default_hair_style")]
    pub hair_style: HairStyle,
    /// brown | blue | green | hazel | gray | amber
    #[serde(default = "default_eye_color")]
    pub eye_color: EyeColor,
    /// very_light | light | medium_light | medium | medium_dark | dark | very_dark
    #[serde(default)]
    pub skin_tone: Option<SkinTone>,
    /// none | stubble | mustache | goatee | full_beard | long_beard
    #[serde(default = "default_facial_hair")]
    pub facial_hair: FacialHair,
    /// neutral | happy | serious | confident | friendly | thoughtful | surprised
    #[serde(default = "default_expression")]
    pub expression: Expression,
    /// Optional list: glasses | sunglasses | earrings | nose_ring | headband |
    /// hat | hijab | turban | necklace | scarf
    #[serde(default)]
    pub accessories: Vec<Accessory>,
    /// white | gray | blue | gradient | nature | studio
    #[serde(default = "default_background")]
    pub background: Background,
    /// photorealistic | digital_art | anime | cartoon | watercolor | oil_painting | pixel_art
    #[serde(default = "default_style")]
    pub style: ArtStyle,
    /// png | jpeg | webp
    #[serde(default = "default_format")]
    pub format: ImageFormat,
    /// Output size in pixels (128–1500, rounded to nearest multiple of 64)
    #[serde(default)]
    pub size: Option<usize>,
    /// Optional seed for reproducible output
    #[serde(default)]
    pub seed: Option<u64>,
    /// headshot | body  (default: headshot)
    /// headshot = tight face+shoulders crop (square canvas)
    /// body     = half-body shot (portrait 3:4 canvas)
    #[serde(default = "default_shot_type")]
    pub shot_type: ShotType,
}

// ── Enums ────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Serialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum Age {
    Baby,
    Toddler,
    Child,
    Teenager,
    YoungAdult,
    Adult,
    MiddleAged,
    Senior,
    Elderly,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum Sex {
    Male,
    Female,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum Ethnicity {
    Caucasian,
    African,
    EastAsian,
    SouthAsian,
    SoutheastAsian,
    Hispanic,
    MiddleEastern,
    NativeAmerican,
    PacificIslander,
    Mixed,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum HairColor {
    Black,
    Brown,
    Blonde,
    Red,
    Gray,
    White,
    Auburn,
    StrawberryBlonde,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum HairStyle {
    Bald,
    BuzzCut,
    Short,
    Medium,
    LongStraight,
    LongWavy,
    LongCurly,
    Afro,
    Braids,
    Ponytail,
    Bun,
    Mohawk,
    Dreadlocks,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum EyeColor {
    Brown,
    Blue,
    Green,
    Hazel,
    Gray,
    Amber,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum SkinTone {
    VeryLight,
    Light,
    MediumLight,
    Medium,
    MediumDark,
    Dark,
    VeryDark,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum FacialHair {
    None,
    Stubble,
    Mustache,
    Goatee,
    FullBeard,
    LongBeard,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum Expression {
    Neutral,
    Happy,
    Serious,
    Confident,
    Friendly,
    Thoughtful,
    Surprised,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Accessory {
    Glasses,
    Sunglasses,
    Earrings,
    NoseRing,
    Headband,
    Hat,
    Hijab,
    Turban,
    Necklace,
    Scarf,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum Background {
    White,
    Gray,
    Blue,
    Gradient,
    Nature,
    Studio,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum ArtStyle {
    Photorealistic,
    DigitalArt,
    Anime,
    Cartoon,
    Watercolor,
    OilPainting,
    PixelArt,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum ImageFormat {
    Png,
    Jpeg,
    Webp,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum ShotType {
    /// Tight crop: face and shoulders only (square canvas)
    Headshot,
    /// Half-body or full-body shot (portrait 3:4 canvas)
    Body,
}

// ── Defaults ─────────────────────────────────────────────────────────────────

fn default_hair_color() -> HairColor {
    HairColor::Brown
}
fn default_hair_style() -> HairStyle {
    HairStyle::Medium
}
fn default_eye_color() -> EyeColor {
    EyeColor::Brown
}
fn default_facial_hair() -> FacialHair {
    FacialHair::None
}
fn default_expression() -> Expression {
    Expression::Neutral
}
fn default_background() -> Background {
    Background::White
}
fn default_style() -> ArtStyle {
    ArtStyle::Photorealistic
}
fn default_format() -> ImageFormat {
    ImageFormat::Png
}
fn default_shot_type() -> ShotType {
    ShotType::Headshot
}

// ── Prompt builder ───────────────────────────────────────────────────────────

impl AvatarRequest {
    /// Converts structured parameters into a Stable Diffusion prompt string.
    pub fn to_prompt(&self) -> String {
        let mut parts: Vec<String> = Vec::new();

        // Style + framing prefix (shot-type aware)
        parts.push(self.framing_prefix().into());

        // Subject: age + ethnicity + sex
        parts.push(format!("of {} {} {}", self.age_text(), self.ethnicity_text(), self.sex_text()));

        // Hair
        if !matches!(self.hair_style, HairStyle::Bald) {
            parts.push(format!("with {} {} hair", self.hair_style_text(), self.hair_color_text()));
        } else {
            parts.push("with a bald head".into());
        }

        // Eyes
        parts.push(format!("{} eyes", self.eye_color_text()));

        // Skin tone
        if let Some(tone) = &self.skin_tone {
            parts.push(format!("{} skin tone", skin_tone_text(tone)));
        }

        // Expression
        parts.push(format!("{} expression", self.expression_text()));

        // Facial hair
        match self.facial_hair {
            FacialHair::None => {}
            _ => parts.push(self.facial_hair_text().into()),
        }

        // Accessories
        if !self.accessories.is_empty() {
            let acc: Vec<&str> = self.accessories
                .iter()
                .map(|a| accessory_text(a))
                .collect();
            parts.push(format!("wearing {}", acc.join(" and ")));
        }

        // Background
        parts.push(format!("{} background", self.background_text()));

        // Quality suffix
        parts.push(self.quality_suffix().into());

        parts.join(", ")
    }

    /// Fixed negative prompt to avoid common SD artifacts.
    pub fn negative_prompt(&self) -> &'static str {
        "ugly, deformed, disfigured, blurry, bad anatomy, extra limbs, mutated, \
         duplicate, morbid, mutilated, poorly drawn face, extra fingers, fused fingers, \
         too many fingers, long neck, watermark, text, signature, logo, banner, \
         low quality, worst quality, normal quality, jpeg artifacts, cropped, \
         asymmetrical eyes, crossed eyes, lazy eye, misaligned eyes, uneven eyes, \
         wall-eyed, off-center eyes, different sized pupils, \
         out of frame, cropped face, partial face, face cut off, face out of frame, \
         head cut off, decapitated, off-center subject, subject not centered"
    }

    // ── Private helpers ──────────────────────────────────────────────────────

    /// Shot-type-aware framing + style prefix.
    fn framing_prefix(&self) -> &str {
        match (&self.style, &self.shot_type) {
            (ArtStyle::Photorealistic, ShotType::Headshot) =>
                "A photorealistic professional headshot portrait photograph, \
                 face and shoulders, centered composition, face centered in frame",
            (ArtStyle::Photorealistic, ShotType::Body) =>
                "A photorealistic professional portrait photograph, \
                 half-body shot, subject centered in frame, centered composition",
            (ArtStyle::DigitalArt, ShotType::Headshot) =>
                "A high-quality digital art headshot portrait, face centered in frame",
            (ArtStyle::DigitalArt, ShotType::Body) =>
                "A high-quality digital art portrait, half-body shot, subject centered",
            (ArtStyle::Anime, ShotType::Headshot) =>
                "An anime-style headshot portrait illustration, face centered",
            (ArtStyle::Anime, ShotType::Body) =>
                "An anime-style portrait illustration, half-body, subject centered",
            (ArtStyle::Cartoon, ShotType::Headshot) =>
                "A cartoon-style headshot portrait, face centered in frame",
            (ArtStyle::Cartoon, ShotType::Body) =>
                "A cartoon-style portrait illustration, half-body, subject centered",
            _ => self.style_prefix(),
        }
    }

    fn style_prefix(&self) -> &str {
        match self.style {
            ArtStyle::Photorealistic => "A photorealistic professional portrait photograph",
            ArtStyle::DigitalArt => "A high-quality digital art portrait",
            ArtStyle::Anime => "An anime-style portrait illustration",
            ArtStyle::Cartoon => "A cartoon-style portrait illustration",
            ArtStyle::Watercolor => "A watercolor painting portrait",
            ArtStyle::OilPainting => "An oil painting portrait",
            ArtStyle::PixelArt => "A pixel art portrait",
        }
    }

    fn age_text(&self) -> &str {
        match self.age {
            Age::Baby => "a baby",
            Age::Toddler => "a toddler",
            Age::Child => "a child",
            Age::Teenager => "a teenage",
            Age::YoungAdult => "a young adult",
            Age::Adult => "an adult",
            Age::MiddleAged => "a middle-aged",
            Age::Senior => "a senior",
            Age::Elderly => "an elderly",
        }
    }

    fn sex_text(&self) -> &str {
        match (&self.sex, &self.age) {
            (Sex::Male, Age::Baby | Age::Toddler | Age::Child) => "boy",
            (Sex::Female, Age::Baby | Age::Toddler | Age::Child) => "girl",
            (Sex::Male, _) => "man",
            (Sex::Female, _) => "woman",
        }
    }

    fn ethnicity_text(&self) -> &str {
        match self.ethnicity {
            Ethnicity::Caucasian => "Caucasian",
            Ethnicity::African => "African",
            Ethnicity::EastAsian => "East Asian",
            Ethnicity::SouthAsian => "South Asian",
            Ethnicity::SoutheastAsian => "Southeast Asian",
            Ethnicity::Hispanic => "Hispanic",
            Ethnicity::MiddleEastern => "Middle Eastern",
            Ethnicity::NativeAmerican => "Native American",
            Ethnicity::PacificIslander => "Pacific Islander",
            Ethnicity::Mixed => "mixed-ethnicity",
        }
    }

    fn hair_color_text(&self) -> &str {
        match self.hair_color {
            HairColor::Black => "black",
            HairColor::Brown => "brown",
            HairColor::Blonde => "blonde",
            HairColor::Red => "red",
            HairColor::Gray => "gray",
            HairColor::White => "white",
            HairColor::Auburn => "auburn",
            HairColor::StrawberryBlonde => "strawberry blonde",
        }
    }

    fn hair_style_text(&self) -> &str {
        match self.hair_style {
            HairStyle::Bald => "bald",
            HairStyle::BuzzCut => "buzz-cut",
            HairStyle::Short => "short",
            HairStyle::Medium => "medium-length",
            HairStyle::LongStraight => "long straight",
            HairStyle::LongWavy => "long wavy",
            HairStyle::LongCurly => "long curly",
            HairStyle::Afro => "afro-styled",
            HairStyle::Braids => "braided",
            HairStyle::Ponytail => "ponytail",
            HairStyle::Bun => "bun-styled",
            HairStyle::Mohawk => "mohawk-styled",
            HairStyle::Dreadlocks => "dreadlocked",
        }
    }

    fn eye_color_text(&self) -> &str {
        match self.eye_color {
            EyeColor::Brown => "brown",
            EyeColor::Blue => "blue",
            EyeColor::Green => "green",
            EyeColor::Hazel => "hazel",
            EyeColor::Gray => "gray",
            EyeColor::Amber => "amber",
        }
    }

    fn expression_text(&self) -> &str {
        match self.expression {
            Expression::Neutral => "neutral",
            Expression::Happy => "happy smiling",
            Expression::Serious => "serious",
            Expression::Confident => "confident",
            Expression::Friendly => "friendly warm",
            Expression::Thoughtful => "thoughtful",
            Expression::Surprised => "surprised",
        }
    }

    fn facial_hair_text(&self) -> &str {
        match self.facial_hair {
            FacialHair::None => "",
            FacialHair::Stubble => "light stubble",
            FacialHair::Mustache => "a mustache",
            FacialHair::Goatee => "a goatee",
            FacialHair::FullBeard => "a full beard",
            FacialHair::LongBeard => "a long beard",
        }
    }

    fn background_text(&self) -> &str {
        match self.background {
            Background::White => "clean white",
            Background::Gray => "neutral gray",
            Background::Blue => "soft blue",
            Background::Gradient => "smooth gradient",
            Background::Nature => "natural outdoors bokeh",
            Background::Studio => "professional studio lighting",
        }
    }

    fn quality_suffix(&self) -> &str {
        match (&self.style, &self.shot_type) {
            (ArtStyle::Photorealistic, ShotType::Headshot) =>
                "highly detailed, sharp focus, professional headshot, studio lighting, \
                 perfect facial features, even lighting",
            (ArtStyle::Photorealistic, ShotType::Body) =>
                "highly detailed, sharp focus, professional portrait, studio lighting, \
                 full body visible, perfect proportions",
            (ArtStyle::Photorealistic, _) =>
                "highly detailed, sharp focus, professional headshot, studio lighting",
            (ArtStyle::DigitalArt, _) => "trending on artstation, highly detailed, smooth",
            (ArtStyle::Anime, _) => "detailed anime style, clean lines, vibrant colors",
            (ArtStyle::Cartoon, _) => "clean vector style, bold outlines, vibrant",
            (ArtStyle::Watercolor, _) => "soft brush strokes, flowing colors, artistic",
            (ArtStyle::OilPainting, _) => "rich oil textures, dramatic lighting, classical",
            (ArtStyle::PixelArt, _) => "retro pixel art, 16-bit style, clean pixels",
        }
    }
}

fn skin_tone_text(tone: &SkinTone) -> &str {
    match tone {
        SkinTone::VeryLight => "very light",
        SkinTone::Light => "light",
        SkinTone::MediumLight => "medium light",
        SkinTone::Medium => "medium",
        SkinTone::MediumDark => "medium dark",
        SkinTone::Dark => "dark",
        SkinTone::VeryDark => "very dark",
    }
}

fn accessory_text(a: &Accessory) -> &str {
    match a {
        Accessory::Glasses => "glasses",
        Accessory::Sunglasses => "sunglasses",
        Accessory::Earrings => "earrings",
        Accessory::NoseRing => "a nose ring",
        Accessory::Headband => "a headband",
        Accessory::Hat => "a hat",
        Accessory::Hijab => "a hijab",
        Accessory::Turban => "a turban",
        Accessory::Necklace => "a necklace",
        Accessory::Scarf => "a scarf",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_photorealistic_prompt() {
        let req = AvatarRequest {
            age: Age::YoungAdult,
            sex: Sex::Female,
            ethnicity: Ethnicity::EastAsian,
            hair_color: HairColor::Black,
            hair_style: HairStyle::LongStraight,
            eye_color: EyeColor::Brown,
            skin_tone: Some(SkinTone::MediumLight),
            facial_hair: FacialHair::None,
            expression: Expression::Happy,
            accessories: vec![Accessory::Glasses],
            background: Background::White,
            style: ArtStyle::Photorealistic,
            format: ImageFormat::Png,
            size: None,
            seed: None,
        };

        let prompt = req.to_prompt();
        assert!(prompt.contains("East Asian"));
        assert!(prompt.contains("woman"));
        assert!(prompt.contains("long straight"));
        assert!(prompt.contains("glasses"));
        assert!(prompt.contains("happy"));
    }
}
