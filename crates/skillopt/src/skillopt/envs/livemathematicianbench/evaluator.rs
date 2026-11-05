fn main() {
    "LiveMathematicianBench evaluation helpers.";
    fn extract_answer(text: String) -> String {
        let mut matches = re
            .findall("<answer>(.*?)</answer>", text, re.DOTALL | re.IGNORECASE);
        if matches {
            return matches[-1].strip();
        }
        let mut lines = text
            .strip()
            .splitlines()
            .into_iter()
            .filter_map(|ln| { if ln.strip() { Some(ln.strip()) } else { None } })
            .collect::<Vec<_>>();
        if lines {
            return lines[-1];
        }
        return text.strip();
    }
    fn normalize_label(text: String) -> String {
        return text.to_string().strip().upper().rstrip(".):");
    }
    fn parse_choice_label(prediction_text: String, choices: list<dict>) -> String {
        let mut answer = extract_answer(prediction_text);
        let mut label = normalize_label(answer);
        let mut valid_labels = choices
            .into_iter()
            .map(|choice| { normalize_label(choice.get("label", "")) })
            .collect::<HashSet<_, _>>();
        if valid_labels.into_iter().any(|v| v == label) {
            return label;
        }
        let mut answer_lower = answer.lower();
        for choice in choices {
            let mut choice_label = normalize_label(choice.get("label", ""));
            let mut choice_text = choice.get("text", "").to_string().strip();
            if choice_text && choice_text.lower() == answer_lower {
                return choice_label;
            }
        }
        let mut first_token = if answer.split() {
            normalize_label(answer.split()[0])
        } else {
            ""
        };
        if valid_labels.into_iter().any(|v| v == first_token) {
            return first_token;
        }
        return label;
    }
    fn evaluate(
        prediction_text: String,
        correct_choice: dict,
        choices: list<dict>,
    ) -> dict {
        let mut predicted_label = parse_choice_label(prediction_text, choices);
        let mut correct_label = normalize_label(correct_choice.get("label", ""));
        let mut predicted_text = "";
        let mut correct_text = correct_choice.get("text", "").to_string().strip();
        for choice in choices {
            if normalize_label(choice.get("label", "")) == predicted_label {
                let mut predicted_text = choice.get("text", "").to_string().strip();
                break;
            }
        }
        let mut is_correct = ((predicted_label == correct_label) as f64);
        return [
            "em",
            "f1",
            "sub_em",
            "predicted_answer",
            "predicted_label",
            "predicted_text",
            "correct_label",
            "correct_text",
        ]
            .into_iter()
            .zip(
                [
                    is_correct,
                    is_correct,
                    is_correct,
                    predicted_label || extract_answer(prediction_text),
                    predicted_label,
                    predicted_text,
                    correct_label,
                    correct_text,
                ]
                    .into_iter(),
            )
            .collect::<HashMap<_, _>>();
    }
}
