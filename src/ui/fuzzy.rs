use std::{borrow::Cow, io::IsTerminal, sync::Arc};

use skim::{
    Skim, SkimItem,
    prelude::{SkimOptionsBuilder, unbounded},
};

use crate::error::{self, Result};

struct StringItem(String);

impl SkimItem for StringItem {
    fn text(&self) -> Cow<'_, str> { Cow::Borrowed(&self.0) }

    fn output(&self) -> Cow<'_, str> { Cow::Borrowed(&self.0) }
}

pub fn select(prompt: &str, items: &[String]) -> Result<Option<String>> {
    if items.is_empty() {
        return Err(error::Error::NoPickerCandidates);
    }
    if !std::io::stdin().is_terminal() || items.len() == 1 {
        return Ok(None);
    }

    let options = SkimOptionsBuilder::default()
        .prompt(prompt)
        .height("100%")
        .multi(false)
        .build()
        .map_err(|source| error::Error::BuildFuzzyFinder { message: source.to_string() })?;

    let items = items
        .iter()
        .map(|item| -> Arc<dyn SkimItem> { Arc::new(StringItem(item.clone())) })
        .collect::<Vec<_>>();
    let (sender, receiver) = unbounded::<Vec<Arc<dyn SkimItem>>>();
    let _unused = sender.send(items);
    drop(sender);

    let output = Skim::run_with(options, Some(receiver))
        .map_err(|source| error::Error::RunFuzzyFinder { message: source.to_string() })?;
    if output.is_abort {
        return Ok(None);
    }

    Ok(output.selected_items.first().map(|item| item.output().into_owned()))
}

pub fn select_or_default(prompt: &str, items: &[String], default: &str) -> Result<String> {
    if items.is_empty() {
        return Err(error::Error::NoPickerCandidates);
    }
    if items.len() == 1 {
        return Ok(items[0].clone());
    }
    Ok(select(prompt, items)?.unwrap_or_else(|| default.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::{select, select_or_default};
    use crate::error::Error;

    #[test]
    fn select_rejects_empty_candidates() {
        assert!(matches!(select("Pick: ", &[]), Err(Error::NoPickerCandidates)));
    }

    #[test]
    fn select_skips_picker_for_single_candidate() {
        assert_eq!(select("Pick: ", &["only".into()]).expect("select"), None);
    }

    #[test]
    fn select_or_default_uses_default_without_tty() {
        let items = vec!["alpha".into(), "beta".into()];
        assert_eq!(select_or_default("Pick: ", &items, "beta").expect("select"), "beta");
    }

    #[test]
    fn select_or_default_prefers_the_only_candidate() {
        assert_eq!(
            select_or_default("Pick: ", &["only".into()], "default").expect("select"),
            "only"
        );
    }
}
