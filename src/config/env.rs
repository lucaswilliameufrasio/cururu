use anyhow::anyhow;
use globset::{Glob, GlobSet, GlobSetBuilder};
use std::env;

pub(super) fn env_required(name: &str) -> anyhow::Result<String> {
    let val = env::var(name).map_err(|_| anyhow!("missing env var {name}"))?;
    if val.is_empty() {
        anyhow::bail!("env var {name} is set but empty");
    }
    Ok(val)
}

pub(super) fn env_optional(name: &str) -> Option<String> {
    let val = env::var(name).ok()?;
    if val.is_empty() { None } else { Some(val) }
}

pub(super) fn env_parse<T>(name: &str, default: T) -> anyhow::Result<T>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    env_optional(name).map_or_else(
        || Ok(default),
        |value| {
            value
                .parse::<T>()
                .map_err(|err| anyhow!("invalid {name}: {err}"))
        },
    )
}

pub(super) fn build_globs(csv: &str) -> anyhow::Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for raw in csv.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        builder.add(Glob::new(raw)?);
    }
    Ok(builder.build()?)
}
