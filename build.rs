use shadow_rs::{BuildPattern, ShadowBuilder};

fn main() {
    let _shadow = ShadowBuilder::builder()
        .build_pattern(BuildPattern::RealTime)
        .build()
        .expect("generate build metadata");
}
