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
    if !std::io::stdin().is_terminal() || items.len() <= 1 {
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
