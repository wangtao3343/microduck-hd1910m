//! What a transcript means, and nothing else.
//!
//! The recognizer emits a sentence with no spaces and no punctuation it can be trusted
//! about; this module decides whether it is a command. It holds no state and opens no
//! sockets, so the whole vocabulary is testable against strings.
//!
//! **Matching is containment after normalisation, not equality.** The streaming model emits
//! a plausible sentence around the words it heard — "嗯坐下吧" and "坐下" both arrive — so
//! equality would miss most real utterances while containment would accept "别坐下" as sit.
//! The compromise is a vocabulary of phrases that are unambiguous inside a sentence, checked
//! in priority order, with [`Command::Stop`] first because a false stop is cheap and a
//! missed stop is not.
//!
//! **Deliberately absent: "开始"/"解锁".** Starting the policy is the gamepad's job. A
//! microphone that can energise the joints is a microphone that can energise them because a
//! television said something, and there is no way for the person holding the duck to know
//! that happened. Everything here assumes a duck that is already enabled and standing.

/// One thing a sentence can ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// Zero the velocity now. Also ends a [`Command::Forward`] early.
    Stop,
    /// The sit↔stand network, one toggle either way. The robot has no separate "sit" and
    /// "stand" skills — `sit_toggle` is what `robot.do` answers to — so "坐下" while already
    /// seated stands the duck up. Stated here because it is the one surprise in the
    /// vocabulary.
    SitToggle,
    /// Walk forward for a bounded time, then stop on our own.
    ///
    /// Bounded rather than "keep walking until told" because the deadman only guards the
    /// last 500 ms: a client that stops refreshing is a robot that stops, so a single
    /// "前进" that armed continuous motion would need this process to stay healthy for as
    /// long as the robot walks. A second and a half is a step, and a second "前进" is the
    /// natural way to ask for another.
    Forward,
    /// The mouth-trigger quack.
    Quack,
}

/// How fast and how long [`Command::Forward`] walks.
///
/// `0.3` m/s rather than a gentler number because the trained walking policy has a dead
/// zone below roughly 0.2 m/s — commands under it produce the stance, not steps. 1.5 s is
/// about one stride cycle at the policy's cadence.
pub const FORWARD_SPEED: f64 = 0.3;
pub const FORWARD_SECONDS: f64 = 1.5;

/// The phrases, in priority order. First match wins.
const VOCABULARY: &[(Command, &[&str])] = &[
    (
        Command::Stop,
        &["停下", "停止", "别动", "站住", "不要动", "停下来", "停"],
    ),
    (
        Command::SitToggle,
        &["坐下", "坐下来", "坐一下", "站起来", "站起", "起立", "起身"],
    ),
    (
        Command::Forward,
        &["往前走", "向前走", "前进", "向前", "走过来", "走两步", "往前"],
    ),
    (
        Command::Quack,
        &["叫一声", "叫一下", "叫唤", "嘎一声", "说句话"],
    ),
];

/// What the sentence means, or `None` for "not a command".
pub fn parse(text: &str) -> Option<Command> {
    let normalized = normalize(text);
    if normalized.is_empty() {
        return None;
    }
    VOCABULARY
        .iter()
        .find(|(_, phrases)| phrases.iter().any(|p| normalized.contains(p)))
        .map(|(command, _)| *command)
}

/// The vocabulary as sherpa-onnx hotwords: one phrase per line, fed to the beam search.
///
/// Measured on the board, not guessed at: with plain greedy search the 14M model
/// transcribed 叫一声 as 叫医生 — the same pinyin, the wrong characters, and so no match —
/// and 叫一声 as 下一声. A hotword is a per-token score bonus for the tokens of the phrase
/// while they are being decoded, which is exactly the lever for "the acoustics are close,
/// the characters are not".
///
/// **Single-character phrases are left out.** 停 is in the vocabulary, but a one-character
/// hotword is one token of bonus and every 听/挺/厅 in the room pays for it. The others are
/// covered by 停下 in any case.
pub fn hotwords() -> String {
    let mut text = String::new();
    for (_, phrases) in VOCABULARY {
        for phrase in phrases.iter().filter(|p| p.chars().count() > 1) {
            text.push_str(phrase);
            text.push('\n');
        }
    }
    text
}

/// Drop everything that is not a meaningful character: whitespace, ASCII punctuation, and the
/// full-width forms the recogniser emits. Case-folded so an English "OK"-style token could
/// never collide by accident.
fn normalize(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_whitespace() && !is_punctuation(*c))
        .flat_map(char::to_lowercase)
        .collect()
}

fn is_punctuation(c: char) -> bool {
    c.is_ascii_punctuation()
        || matches!(
            c,
            '，' | '。'
                | '！'
                | '？'
                | '、'
                | '；'
                | '：'
                | '“'
                | '”'
                | '‘'
                | '’'
                | '（'
                | '）'
                | '【'
                | '】'
                | '《'
                | '》'
                | '…'
                | '—'
                | '·'
                | '～'
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_plain_utterance_is_a_command() {
        assert_eq!(parse("坐下"), Some(Command::SitToggle));
        assert_eq!(parse("前进"), Some(Command::Forward));
        assert_eq!(parse("停"), Some(Command::Stop));
        assert_eq!(parse("叫一声"), Some(Command::Quack));
    }

    #[test]
    fn punctuation_and_spaces_do_not_matter() {
        // What the stream actually emits: no spaces, but a trailing comma is common.
        assert_eq!(parse("坐下，"), Some(Command::SitToggle));
        assert_eq!(parse(" 前进。"), Some(Command::Forward));
        assert_eq!(parse("叫、一、声"), Some(Command::Quack));
    }

    #[test]
    fn the_sentence_around_the_word_does_not_matter() {
        assert_eq!(parse("嗯，快坐下吧"), Some(Command::SitToggle));
        assert_eq!(parse("鸭子向前走两步"), Some(Command::Forward));
        assert_eq!(parse("你给我停下来"), Some(Command::Stop));
    }

    #[test]
    fn stop_wins_when_a_sentence_holds_two_commands() {
        // "往前走，停下" — the safe reading is the stop.
        assert_eq!(parse("往前走停下"), Some(Command::Stop));
    }

    #[test]
    fn the_ducks_own_prompts_are_not_commands() {
        // Every generated prompt, because the speaker feeds the microphone: the duck will
        // hear itself. Only "我累了，休息一下" contains anything close to a verb at all, and
        // it deliberately maps to nothing — relax is the gamepad's Select release.
        for prompt in [
            "开机完成",
            "准备",
            "解锁",
            "锁定",
            "我累了休息一下",
            "电量低",
            "我快要噶了立即给我充电",
            "伺服故障请检查",
            "伺服故障已复位请按开始键继续",
            "正在关机",
            "电量耗尽即将关机",
        ] {
            assert_eq!(parse(prompt), None, "prompt {prompt:?} parsed as a command");
        }
    }

    #[test]
    fn unrelated_speech_is_not_a_command() {
        assert_eq!(parse("今天天气不错"), None);
        assert_eq!(parse(""), None);
        assert_eq!(parse("......"), None);
    }

    #[test]
    fn every_multi_character_phrase_is_a_hotword() {
        // The decoder and the matcher must agree on what the phrases are; if they drift,
        // the hotword biases toward something `parse` would not accept.
        let hotwords = hotwords();
        for (_, phrases) in VOCABULARY {
            for phrase in *phrases {
                let listed = hotwords.lines().any(|line| line == *phrase);
                assert_eq!(
                    listed,
                    phrase.chars().count() > 1,
                    "phrase {phrase:?} listed = {listed}"
                );
            }
        }
        // And the hotword file's lines really are the vocabulary, not a superset.
        for line in hotwords.lines() {
            assert!(parse(line).is_some(), "hotword {line:?} is not a command");
        }
    }
}
