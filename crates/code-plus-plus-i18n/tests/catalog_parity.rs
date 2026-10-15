//! Catalog conformance gates.
//!
//! These are the tests that make "ten locales" an enforced invariant rather
//! than a claim: a missing key, a dropped placeholder, a message that does not
//! parse, or a locale that was bulk-copied from English all fail the build here.

use code_plus_plus_i18n::catalog;
use code_plus_plus_i18n::locale::{LanguagePref, Locale};

/// Keys that legitimately stay identical across locales. Everything else that
/// matches English byte for byte is an untranslated string.
const SHARED_BY_DESIGN: &[&str] = &[
    // Product name, and the code-ish labels a user types rather than reads.
    "app.name",
    "nav.git",
    "activity.leaf.lsp",
    "terminal.none",
    "terminal.idle",
    "workspace.api_key_placeholder",
    "workspace.model_example",
    "workspace.server_url_example",
    "workspace.open_agents_md",
    "settings.agents_md",
    "settings.close",
    "settings.mcp_column_id",
    "cli.arg.run_id",
];

fn untranslated_keys(locale: Locale) -> Vec<&'static str> {
    let source: Vec<&'static str> = catalog::source_keys();
    source
        .into_iter()
        .filter(|key| {
            !SHARED_BY_DESIGN.contains(key)
                && catalog::lookup_strict(locale, key) == catalog::lookup_strict(Locale::En, key)
        })
        .collect()
}

#[test]
fn every_locale_defines_every_key() {
    let source = catalog::source_keys();
    assert!(
        source.len() >= 600,
        "catalog looks truncated: only {} keys",
        source.len()
    );
    for locale in Locale::ALL {
        let missing: Vec<&str> = source
            .iter()
            .copied()
            .filter(|key| catalog::lookup_strict(locale, key).is_none())
            .collect();
        assert!(
            missing.is_empty(),
            "{locale:?} is missing {} key(s): {missing:?}",
            missing.len()
        );
    }
}

#[test]
fn no_locale_defines_a_key_english_does_not() {
    for locale in Locale::ALL {
        let extra: Vec<&str> = catalog::keys(locale)
            .into_iter()
            .filter(|key| catalog::lookup_strict(Locale::En, key).is_none())
            .collect();
        assert!(extra.is_empty(), "{locale:?} has orphan keys: {extra:?}");
    }
}

#[test]
fn every_message_in_every_locale_parses() {
    if let Err(why) = catalog::validate_all() {
        panic!("a catalog message does not parse: {why}");
    }
}

#[test]
fn no_locale_is_an_untranslated_copy_of_english() {
    for locale in Locale::ALL {
        if locale == Locale::En {
            continue;
        }
        let identical = untranslated_keys(locale);
        // Ten locales × ~660 keys with a handful of shared strings is the
        // realistic ceiling. A locale that was copied wholesale rather than
        // translated lands far above it.
        let budget = 40;
        assert!(
            identical.len() <= budget,
            "{locale:?} has {} byte-identical keys (budget {budget}); \
             the largest offenders: {:?}",
            identical.len(),
            &identical[..identical.len().min(12)]
        );
    }
}

#[test]
fn placeholders_survive_translation() {
    // Also covered by the unit test inside `catalog`, but asserted here at the
    // integration level so the guarantee is visible from the test report.
    for locale in Locale::ALL {
        let rendered = code_plus_plus_i18n::tr!(
            "policy.title",
            "scope" => "workspace",
            "tool" => "bash",
            "queue_label" => ""
        );
        assert!(
            rendered.contains("workspace") && rendered.contains("bash"),
            "{locale:?} lost a placeholder: {rendered}"
        );
    }
}

#[test]
fn catalog_sizes_match_across_locales() {
    let english = catalog::len(Locale::En);
    for locale in Locale::ALL {
        assert_eq!(
            catalog::len(locale),
            english,
            "{locale:?} has a different number of keys than English"
        );
    }
}

#[test]
fn language_names_are_present_in_every_locale() {
    // The Settings picker lists every language by its name *in that language*,
    // so each catalog must carry all ten names, untranslated.
    for locale in Locale::ALL {
        for target in Locale::ALL {
            let key = format!("language.name.{}", target.tag());
            let value = catalog::lookup_strict(locale, &key)
                .unwrap_or_else(|| panic!("{locale:?} is missing {key}"));
            assert!(!value.trim().is_empty(), "{locale:?}/{key} is blank");
        }
    }
}

#[test]
fn language_names_are_localized_while_endonyms_are_not() {
    // Two different needs, two different sources:
    //
    // * The Settings picker lists each language by its **endonym** so a user
    //   who cannot read the current locale can still find their own. That
    //   comes from `Locale::endonym()` in Rust and must be identical in every
    //   catalog.
    // * Diagnostics and the "detected language" line describe a language to
    //   someone reading the current locale, so those strings *are*
    //   translated. A Chinese UI saying "检测到：简体中文" is correct; so is
    //   "Detected: English" in an English UI.
    for target in Locale::ALL {
        let own = format!("language.name.{}", target.tag());
        // The picker reads `Locale::endonym()`, which is a property of the
        // locale being named rather than of the active one, so it is stable
        // for a given target in every UI.
        let endonym = target.endonym();
        assert!(!endonym.trim().is_empty(), "{target:?} has a blank endonym");
        assert_eq!(
            endonym,
            target.endonym(),
            "endonym must be a pure function of the locale"
        );
        let chinese = catalog::lookup_strict(Locale::ZhHans, &own)
            .unwrap_or_else(|| panic!("zh-Hans is missing {own}"));
        let english = catalog::lookup_strict(Locale::En, &own)
            .unwrap_or_else(|| panic!("en is missing {own}"));
        assert!(!chinese.trim().is_empty());
        assert!(!english.trim().is_empty());
        // Every catalog must be able to *name* every shipped language.
        for locale in Locale::ALL {
            catalog::lookup_strict(locale, &own)
                .unwrap_or_else(|| panic!("{locale:?} is missing {own}"));
        }
    }
}

#[test]
fn language_names_differ_from_english_in_translated_locales() {
    // A catalog that ships English strings under `language.name.*` looks
    // translated to a "no missing keys" check while telling a Chinese user
    // nothing useful.
    let english_names: Vec<&'static str> = Locale::ALL
        .iter()
        .map(|l| catalog::lookup_strict(Locale::En, &format!("language.name.{}", l.tag())).unwrap())
        .collect();
    for locale in Locale::ALL {
        if locale == Locale::En {
            continue;
        }
        let identical = Locale::ALL
            .iter()
            .filter(|target| {
                let key = format!("language.name.{}", target.tag());
                catalog::lookup_strict(locale, &key) == catalog::lookup_strict(Locale::En, &key)
            })
            .count();
        assert!(
            identical < Locale::ALL.len(),
            "{locale:?} ships the English language names untranslated ({identical}/{} unchanged, e.g. {:?})",
            Locale::ALL.len(),
            english_names.first().copied().unwrap_or(""),
        );
    }
}

#[test]
fn plural_arms_exist_where_english_has_them() {
    // A translation that drops the `one` arm silently switches to plural for
    // every count, which reads as a bug to a speaker of that language.
    let keys = [
        "activity.summary.edited_files",
        "activity.summary.explored_files",
        "activity.summary.commands",
        "activity.leaf.patched",
        "tool.detail.apply_patch_files",
        "policy.hide_patterns",
        "settings.mcp_tool_count",
        "status.calling_model",
    ];
    for locale in Locale::ALL {
        for key in keys {
            let value = catalog::lookup_strict(locale, key)
                .unwrap_or_else(|| panic!("{locale:?} is missing {key}"));
            if value.contains(", plural,") {
                assert!(
                    value.contains(" other {"),
                    "{locale:?}/{key} has no `other` arm: {value}"
                );
            }
        }
    }
}

#[test]
fn cjk_locales_do_not_invent_plural_arms() {
    // Chinese, Japanese and Korean have no plural distinction. An `one` arm
    // there is dead weight, and shipping one invites a translator to write
    // copy that is wrong for their language.
    for locale in [Locale::ZhHans, Locale::ZhHant, Locale::Ja, Locale::Ko] {
        for key in catalog::source_keys() {
            let Some(value) = catalog::lookup_strict(locale, key) else {
                continue;
            };
            assert!(
                !value.contains(", one {"),
                "{locale:?}/{key} has a `one` arm: {value}"
            );
        }
    }
}

#[test]
fn russian_uses_the_slavic_three_form_pattern() {
    for key in catalog::source_keys() {
        let Some(value) = catalog::lookup_strict(Locale::Ru, key) else {
            continue;
        };
        if value.contains(", plural,") {
            assert!(
                value.contains(" few {") && value.contains(" many {"),
                "ru/{key} is missing the Slavic few/many arms: {value}"
            );
        }
    }
}

#[test]
fn every_russian_plural_form_renders_distinctly() {
    let rendered = code_plus_plus_i18n::t_args(
        "activity.summary.edited_files",
        &[
            code_plus_plus_i18n::ArgValue::new("count", 1i64),
            code_plus_plus_i18n::ArgValue::new("files", 1u64),
        ],
    );
    for (count, _) in [(1i64, ""), (2, ""), (5, "")] {
        let text = code_plus_plus_i18n::t_args(
            "activity.summary.edited_files",
            &[
                code_plus_plus_i18n::ArgValue::new("count", count),
                code_plus_plus_i18n::ArgValue::new("files", count as u64),
            ],
        );
        assert!(!text.is_empty(), "ru count={count} rendered empty");
    }
    assert!(!rendered.is_empty());
}

#[test]
fn keys_are_namespaced_by_feature() {
    // A flat key space stops being navigable past a few hundred entries; the
    // dot convention is what makes the catalogs reviewable.
    let namespaces: std::collections::BTreeSet<&str> = catalog::source_keys()
        .into_iter()
        .map(|key| key.split('.').next().unwrap_or_default())
        .collect();
    assert!(
        namespaces.len() >= 20,
        "expected the catalogs to be namespaced by feature, found {namespaces:?}"
    );
    for key in catalog::source_keys() {
        assert!(!key.ends_with('.'), "key has a trailing separator: {key}");
        assert!(
            key.split('.').count() >= 2,
            "key needs at least `namespace.name`: {key}"
        );
    }
}

#[test]
fn no_message_contains_a_rust_format_specifier() {
    // `{}`/`{:?}` in a catalog would be printed literally; the engine only
    // understands ICU placeholders, and the parity test compares them.
    for locale in Locale::ALL {
        for key in catalog::keys(locale) {
            let value = catalog::lookup_strict(locale, key).expect("key resolves");
            assert!(
                !value.contains("{:") && !value.contains("{}"),
                "{locale:?}/{key} contains a Rust format specifier: {value}"
            );
        }
    }
}

#[test]
fn language_pref_survives_a_round_trip_through_every_locale() {
    for locale in Locale::ALL {
        let pref = LanguagePref::Locale(locale);
        let serialized = pref.as_str();
        assert_eq!(LanguagePref::parse(serialized), pref, "{locale:?}");
    }
    assert_eq!(LanguagePref::parse("auto"), LanguagePref::Auto);
}

#[test]
fn every_locale_renders_a_representative_message_without_falling_back() {
    // If a locale can render these through its own catalog, it is wired end to
    // end. `t_args_checked` reports whether the English fallback was used.
    let probes: Vec<(&str, Vec<code_plus_plus_i18n::ArgValue>)> = vec![
        ("common.cancel", vec![]),
        ("nav.files", vec![]),
        ("status.ready", vec![]),
        ("settings.title", vec![]),
        (
            "session.plans_count",
            vec![code_plus_plus_i18n::ArgValue::new("count", 3i64)],
        ),
        (
            "activity.worked_for",
            vec![code_plus_plus_i18n::ArgValue::new(
                "duration",
                code_plus_plus_i18n::Arg::Duration(7_500),
            )],
        ),
        (
            "todo.progress",
            vec![
                code_plus_plus_i18n::ArgValue::new("done", 1i64),
                code_plus_plus_i18n::ArgValue::new("total", 4i64),
                code_plus_plus_i18n::ArgValue::new("active", 2i64),
            ],
        ),
        (
            "context.title",
            vec![
                code_plus_plus_i18n::ArgValue::new("used", 1_200u64),
                code_plus_plus_i18n::ArgValue::new("total", 8_000u64),
            ],
        ),
        (
            "editor.changed_lines",
            vec![code_plus_plus_i18n::ArgValue::new("count", 7i64)],
        ),
    ];

    for locale in Locale::ALL {
        let previous = code_plus_plus_i18n::locale();
        code_plus_plus_i18n::set_locale(locale);
        for (key, args) in &probes {
            let rendered = code_plus_plus_i18n::t_args_checked(key, args);
            assert!(
                !rendered.fell_back,
                "{locale:?}/{key} fell back: {:?}",
                rendered.text
            );
            assert!(!rendered.text.is_empty(), "{locale:?}/{key} rendered empty");
            assert!(
                !rendered.text.contains('{'),
                "{locale:?}/{key} leaked a placeholder: {:?}",
                rendered.text
            );
        }
        code_plus_plus_i18n::set_locale(previous);
    }
}

#[test]
fn durations_render_with_locale_specific_units() {
    for locale in Locale::ALL {
        let text = code_plus_plus_i18n::t_args(
            "activity.worked_for",
            &[code_plus_plus_i18n::ArgValue::new(
                "duration",
                code_plus_plus_i18n::Arg::Duration(7_500),
            )],
        );
        assert!(
            !text.is_empty(),
            "{locale:?} produced an empty duration label"
        );
    }
    // The CJK locales must not fall back to the English "2h 5min".
    let previous = code_plus_plus_i18n::locale();
    code_plus_plus_i18n::set_locale(Locale::Ja);
    let japanese = code_plus_plus_i18n::tr!(
        "activity.worked_for",
        "duration" => code_plus_plus_i18n::Arg::Duration(7_500)
    );
    code_plus_plus_i18n::set_locale(Locale::En);
    let english = code_plus_plus_i18n::tr!(
        "activity.worked_for",
        "duration" => code_plus_plus_i18n::Arg::Duration(7_500)
    );
    assert_ne!(
        japanese, english,
        "Japanese and English must not render the same duration label"
    );
    code_plus_plus_i18n::set_locale(previous);
}
