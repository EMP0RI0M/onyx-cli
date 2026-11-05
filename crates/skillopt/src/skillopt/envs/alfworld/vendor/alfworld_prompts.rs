fn main() {
    let mut ALFWORLD_TEMPLATE_NO_HIS = load_prompt(
        "rollout_no_history",
        load_promptParams {
            env: "alfworld",
        },
    );
    let mut ALFWORLD_TEMPLATE = load_prompt(
        "rollout_with_history",
        load_promptParams {
            env: "alfworld",
        },
    );
    let mut ALFWORLD_TEMPLATE_WITH_MEMORY = load_prompt(
        "rollout_with_memory",
        load_promptParams {
            env: "alfworld",
        },
    );
}
