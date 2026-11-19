use enzui_iced_core::localization::{CatalogBuilder, Localizer};

#[path = "l10n_keys.rs"]
mod keys;

pub(crate) fn localizer() -> Localizer {
    #[cfg(test)]
    let preferred = vec!["en-US".parse().expect("bundled test locale")];
    #[cfg(not(test))]
    let preferred = enzui_iced_core::localization::preferred_locales();
    let localizer = Localizer::new(
        catalog(),
        preferred,
        "en-US".parse().expect("valid fallback locale"),
    );
    #[cfg(not(test))]
    for category in enzui_iced_core::localization::LocaleCategory::ALL {
        let locale = enzui_iced_core::localization::category_locale_from_env(category, |key| {
            std::env::var(key).ok()
        });
        let _ = localizer.set_category_locale(category, locale);
    }
    localizer
}

fn catalog() -> enzui_iced_core::localization::Catalog {
    let mut catalog = CatalogBuilder::new();
    for (locale, source) in [
        ("en-US", include_str!("../assets/locales/en-US.ftl")),
        ("de", include_str!("../assets/locales/de.ftl")),
        ("ar", include_str!("../assets/locales/ar.ftl")),
    ] {
        catalog
            .add_resource(locale, source)
            .expect("valid bundled translations");
    }
    catalog.build()
}

pub(crate) fn current() -> Localizer {
    #[cfg(not(test))]
    {
        enzui_iced_core::localization::current()
    }
    #[cfg(test)]
    {
        enzui_iced_core::localization::scoped().unwrap_or_else(|| TEST_LOCALIZER.with(Clone::clone))
    }
}

// English labels address stable application message keys; they are never stored translations.
pub(crate) fn tr(source: &str) -> String {
    keys::key(source).map_or_else(|| source.to_owned(), |key| current().text(key))
}

pub(crate) fn rtl() -> bool {
    current().locale().direction() == enzui_iced_core::placement::Direction::RightToLeft
}

pub(crate) fn count(key: &str, count: usize) -> String {
    let mut arguments = enzui_iced_core::localization::Arguments::new();
    arguments.set("count", count);
    current().text_with(key, &arguments)
}

pub(crate) fn relative_time(key: &str, time: String) -> String {
    let mut arguments = enzui_iced_core::localization::Arguments::new();
    arguments.set("time", time);
    current().text_with(key, &arguments)
}

pub(crate) fn date_time(date: String, time: String) -> String {
    let mut arguments = enzui_iced_core::localization::Arguments::new();
    arguments.set("date", date);
    arguments.set("time", time);
    current().text_with("mail-date-time", &arguments)
}

pub(crate) fn message(key: &str, values: &[(&str, String)]) -> String {
    let mut arguments = enzui_iced_core::localization::Arguments::new();
    for (name, value) in values {
        arguments.set(*name, value.clone());
    }
    current().text_with(key, &arguments)
}

pub(crate) fn number(value: usize) -> String {
    let canonical = value.to_string();
    canonical
        .parse::<enzui_iced_core::localization::Decimal>()
        .ok()
        .and_then(|decimal| {
            current()
                .locale_for(enzui_iced_core::localization::LocaleCategory::Numeric)
                .format_decimal(&decimal)
                .ok()
        })
        .unwrap_or(canonical)
}

#[cfg(test)]
thread_local! { static TEST_LOCALIZER: Localizer = localizer(); }
