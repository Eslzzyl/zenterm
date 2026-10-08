use std::collections::HashMap;
use std::ops::Range;

use cosmic_text::skrifa::MetadataProvider;
use cosmic_text::{Fallback, PlatformFallback};
use fontdb::{Database, ID, Style, Weight};
use unicode_script::{Script, UnicodeScript};
#[derive(Clone, Copy)]
struct FaceRecord {
    id: ID,
    weight: Weight,
    style: Style,
    monospaced: bool,
}

pub(crate) struct FontResolver {
    primary_family: String,
    family_faces: HashMap<String, Vec<FaceRecord>>,
    family_order: Vec<String>,
    script_bindings: HashMap<Script, String>,
    glyph_support_cache: HashMap<(ID, u32), bool>,
}

pub(crate) struct ResolvedSpan {
    pub range: Range<usize>,
    pub family: String,
    pub weight: Weight,
}

impl FontResolver {
    pub(crate) fn new(primary_family: impl Into<String>, db: &Database) -> Self {
        let mut family_faces: HashMap<String, Vec<FaceRecord>> = HashMap::new();
        let mut family_order = Vec::new();

        for face in db.faces() {
            let record = FaceRecord {
                id: face.id,
                weight: face.weight,
                style: face.style,
                monospaced: face.monospaced,
            };
            for (family, _) in &face.families {
                if !family_faces.contains_key(family) {
                    family_order.push(family.clone());
                }
                family_faces.entry(family.clone()).or_default().push(record);
            }
        }

        Self {
            primary_family: primary_family.into(),
            family_faces,
            family_order,
            script_bindings: HashMap::new(),
            glyph_support_cache: HashMap::new(),
        }
    }

    pub(crate) fn resolve_spans(
        &mut self,
        db: &Database,
        locale: &str,
        text: &str,
        requested_weight: Weight,
        requested_style: Style,
    ) -> Vec<ResolvedSpan> {
        if text.is_empty() {
            return Vec::new();
        }

        let mut spans = Vec::new();
        let mut current_start = 0;
        let mut current_script = None;

        for (start, character) in text.char_indices() {
            let script = character.script();
            if current_script != Some(script) {
                if let Some(previous_script) = current_script {
                    self.resolve_range(
                        db,
                        locale,
                        text,
                        current_start..start,
                        previous_script,
                        requested_weight,
                        requested_style,
                        &mut spans,
                    );
                }
                current_start = start;
                current_script = Some(script);
            }
        }

        if let Some(script) = current_script {
            self.resolve_range(
                db,
                locale,
                text,
                current_start..text.len(),
                script,
                requested_weight,
                requested_style,
                &mut spans,
            );
        }

        spans
    }

    fn resolve_range(
        &mut self,
        db: &Database,
        locale: &str,
        text: &str,
        range: Range<usize>,
        script: Script,
        requested_weight: Weight,
        requested_style: Style,
        spans: &mut Vec<ResolvedSpan>,
    ) {
        let span_text = &text[range.clone()];
        let (family, weight) = if is_strong_script(script) {
            self.resolve_strong_script(
                db,
                locale,
                span_text,
                script,
                requested_weight,
                requested_style,
            )
        } else {
            (
                self.primary_family.clone(),
                self.family_weight(db, &self.primary_family, requested_weight, requested_style),
            )
        };

        spans.push(ResolvedSpan {
            range,
            family,
            weight,
        });
    }

    fn resolve_strong_script(
        &mut self,
        db: &Database,
        locale: &str,
        text: &str,
        script: Script,
        requested_weight: Weight,
        requested_style: Style,
    ) -> (String, Weight) {
        if let Some(family) = self.script_bindings.get(&script) {
            let family = family.clone();
            let weight = self.family_weight(db, &family, requested_weight, requested_style);
            return (family, weight);
        }

        let family = if family_covers(
            db,
            &self.family_faces,
            &mut self.glyph_support_cache,
            &self.primary_family,
            text,
        ) {
            Some(self.primary_family.clone())
        } else {
            let platform = PlatformFallback;
            let platform_family = platform
                .script_fallback(script, locale)
                .iter()
                .chain(platform.common_fallback().iter())
                .find(|family| {
                    family_covers(
                        db,
                        &self.family_faces,
                        &mut self.glyph_support_cache,
                        family,
                        text,
                    )
                })
                .map(|family| (*family).to_string());

            platform_family.or_else(|| {
                first_covering_family(
                    db,
                    &self.family_faces,
                    &self.family_order,
                    &mut self.glyph_support_cache,
                    text,
                    true,
                )
                .or_else(|| {
                    first_covering_family(
                        db,
                        &self.family_faces,
                        &self.family_order,
                        &mut self.glyph_support_cache,
                        text,
                        false,
                    )
                })
            })
        }
        .unwrap_or_else(|| {
            log::debug!(
                "font fallback unresolved: script={script:?} locale={locale:?} text={text:?}"
            );
            self.primary_family.clone()
        });

        let weight = self.family_weight(db, &family, requested_weight, requested_style);
        self.script_bindings.insert(script, family.clone());
        log::debug!(
            "font fallback binding: script={script:?} locale={locale:?} family={family:?} requested_weight={} selected_weight={}",
            requested_weight.0,
            weight.0,
        );
        (family, weight)
    }

    fn family_weight(
        &self,
        db: &Database,
        family: &str,
        requested_weight: Weight,
        requested_style: Style,
    ) -> Weight {
        let Some(faces) = self.family_faces.get(family) else {
            return requested_weight;
        };

        select_face_weight(faces.iter(), requested_weight, requested_style, |id| {
            variable_weight_range(db, id)
        })
        .map_or(requested_weight, |(weight, _)| weight)
    }
}

fn is_strong_script(script: Script) -> bool {
    !matches!(script, Script::Common | Script::Inherited | Script::Unknown)
}

fn first_covering_family(
    db: &Database,
    family_faces: &HashMap<String, Vec<FaceRecord>>,
    family_order: &[String],
    glyph_support_cache: &mut HashMap<(ID, u32), bool>,
    text: &str,
    monospaced_only: bool,
) -> Option<String> {
    family_order.iter().find_map(|family| {
        let faces = family_faces.get(family)?;
        if monospaced_only && !faces.iter().any(|face| face.monospaced) {
            return None;
        }
        family_covers_with_faces(db, faces, glyph_support_cache, text).then(|| family.clone())
    })
}

fn family_covers(
    db: &Database,
    family_faces: &HashMap<String, Vec<FaceRecord>>,
    glyph_support_cache: &mut HashMap<(ID, u32), bool>,
    family: &str,
    text: &str,
) -> bool {
    family_faces
        .get(family)
        .is_some_and(|faces| family_covers_with_faces(db, faces, glyph_support_cache, text))
}

fn family_covers_with_faces(
    db: &Database,
    faces: &[FaceRecord],
    glyph_support_cache: &mut HashMap<(ID, u32), bool>,
    text: &str,
) -> bool {
    faces.iter().any(|face| {
        text.chars()
            .all(|character| glyph_supported(db, face.id, character, glyph_support_cache))
    })
}

fn glyph_supported(
    db: &Database,
    face_id: ID,
    character: char,
    glyph_support_cache: &mut HashMap<(ID, u32), bool>,
) -> bool {
    let codepoint = u32::from(character);
    if let Some(&supported) = glyph_support_cache.get(&(face_id, codepoint)) {
        return supported;
    }

    let supported = db
        .with_face_data(face_id, |data, index| {
            ttf_parser::Face::parse(data, index)
                .ok()
                .and_then(|face| face.glyph_index(character))
                .is_some()
        })
        .unwrap_or(false);
    glyph_support_cache.insert((face_id, codepoint), supported);
    supported
}

fn variable_weight_range(db: &Database, face_id: ID) -> Option<(f32, f32)> {
    db.with_face_data(face_id, |data, index| {
        let font = cosmic_text::skrifa::FontRef::from_index(data, index).ok()?;
        let axis = font
            .axes()
            .get_by_tag(cosmic_text::skrifa::Tag::new(b"wght"))?;
        Some((axis.min_value(), axis.max_value()))
    })
    .flatten()
}

fn select_face_weight<'a, I, F>(
    faces: I,
    requested_weight: Weight,
    requested_style: Style,
    mut variable_weight_range: F,
) -> Option<(Weight, bool)>
where
    I: IntoIterator<Item = &'a FaceRecord>,
    F: FnMut(ID) -> Option<(f32, f32)>,
{
    let mut selected = None;

    for face in faces {
        let variable_covers = variable_weight_range(face.id).is_some_and(|(min, max)| {
            (requested_weight.0 as f32) >= min && (requested_weight.0 as f32) <= max
        });
        let capable = variable_covers || face.weight == requested_weight;
        let difference = if variable_covers {
            0
        } else {
            face.weight.0.abs_diff(requested_weight.0)
        };
        let score = (
            face.style != requested_style,
            !capable,
            difference,
            face.weight.0,
        );
        let candidate = (
            score,
            if variable_covers {
                requested_weight
            } else {
                face.weight
            },
            variable_covers,
        );

        if selected
            .as_ref()
            .is_none_or(|selected: &(bool, bool, u16, u16, Weight, bool)| {
                (score.0, score.1, score.2, score.3)
                    < (selected.0, selected.1, selected.2, selected.3)
            })
        {
            selected = Some((score.0, score.1, score.2, score.3, candidate.1, candidate.2));
        }
    }

    selected.map(|(_, _, _, _, weight, variable)| (weight, variable))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn face(weight: u16, style: Style) -> FaceRecord {
        FaceRecord {
            id: ID::dummy(),
            weight: Weight(weight),
            style,
            monospaced: true,
        }
    }

    #[test]
    fn static_family_uses_nearest_weight() {
        let faces = [face(400, Style::Normal), face(600, Style::Normal)];
        let selected = select_face_weight(faces.iter(), Weight::BOLD, Style::Normal, |_| None);
        assert_eq!(selected, Some((Weight(600), false)));
    }

    #[test]
    fn static_family_prefers_exact_weight() {
        let faces = [face(400, Style::Normal), face(700, Style::Normal)];
        let selected = select_face_weight(faces.iter(), Weight::BOLD, Style::Normal, |_| None);
        assert_eq!(selected, Some((Weight::BOLD, false)));
    }

    #[test]
    fn variable_family_keeps_requested_weight() {
        let faces = [face(400, Style::Normal)];
        let selected = select_face_weight(faces.iter(), Weight::BOLD, Style::Normal, |_| {
            Some((400.0, 700.0))
        });
        assert_eq!(selected, Some((Weight::BOLD, true)));
    }

    #[test]
    fn style_match_precedes_weight_distance() {
        let faces = [face(700, Style::Italic), face(600, Style::Normal)];
        let selected = select_face_weight(faces.iter(), Weight::BOLD, Style::Normal, |_| None);
        assert_eq!(selected, Some((Weight(600), false)));
    }

    #[test]
    fn installed_fonts_bind_han_to_one_family() {
        let font_system = cosmic_text::FontSystem::new();
        let Some(primary_family) = font_system
            .db()
            .faces()
            .find_map(|face| face.families.first().map(|(family, _)| family.clone()))
        else {
            eprintln!("font resolver test skipped: installed database is empty");
            return;
        };
        let mut resolver = FontResolver::new(primary_family, font_system.db());
        let text = "构询建";

        let bold = resolver.resolve_spans(
            font_system.db(),
            font_system.locale(),
            text,
            Weight::BOLD,
            Style::Normal,
        );
        let bold_again = resolver.resolve_spans(
            font_system.db(),
            font_system.locale(),
            text,
            Weight::BOLD,
            Style::Normal,
        );
        let regular = resolver.resolve_spans(
            font_system.db(),
            font_system.locale(),
            text,
            Weight::NORMAL,
            Style::Normal,
        );

        assert_eq!(bold.len(), 1);
        assert_eq!(bold_again.len(), 1);
        assert_eq!(regular.len(), 1);
        assert!(bold.iter().all(|span| span.family == bold[0].family));
        assert_eq!(bold[0].family, bold_again[0].family);
        assert_eq!(bold[0].family, regular[0].family);

        if let Some(faces) = resolver.family_faces.get(&bold[0].family) {
            let has_variable_bold = faces.iter().any(|face| {
                variable_weight_range(font_system.db(), face.id)
                    .is_some_and(|(min, max)| (min..=max).contains(&(Weight::BOLD.0 as f32)))
            });
            if has_variable_bold {
                assert_eq!(bold[0].weight, Weight::BOLD);
            } else if let Some((expected, _)) =
                select_face_weight(faces.iter(), Weight::BOLD, Style::Normal, |id| {
                    variable_weight_range(font_system.db(), id)
                })
            {
                assert_eq!(bold[0].weight, expected);
            }
        }
    }
}
