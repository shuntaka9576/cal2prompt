use minijinja::{context, Environment};

use crate::core::cal2prompt::Day;

pub fn generate(template: &str, days: Vec<Day>) -> anyhow::Result<String> {
    let mut env = Environment::new();
    env.set_trim_blocks(true);
    env.set_lstrip_blocks(true);

    env.add_template("schedule", template)?;
    let tmpl = env.get_template("schedule")?;

    let rendered = tmpl.render(context! {
        days => days
    })?;

    Ok(rendered)
}
