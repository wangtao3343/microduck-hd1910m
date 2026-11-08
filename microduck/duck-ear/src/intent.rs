//! What a transcript means, and nothing else.
//!
//! The recognizer emits a sentence with no spaces and no punctuation it can be trusted
//! about; this module decides whether it is a command. It holds no state and opens no
//! sockets, so the whole vocabulary is testable against strings.
//!
//! **Matching is containment after normalisation, not equality.** The recognizer emits a
//! plausible sentence around the words it heard — "嗯坐下吧" and "坐下" both arrive — so
//! equality would miss most real utterances while containment would accept "别坐下" as sit.
//! The compromise is a vocabulary of phrases that are unambiguous inside a sentence, checked
//! in priority order, with [`Command::Stop`] first because a false stop is cheap and a
//! missed stop is not.
//!
//! **This layer did not change when the recognizer did.** With the first streaming model,
//! 叫一声 arrived as 叫医生 — the right pinyin, the wrong characters, and so no match — a
//! model problem that hotword biasing only partly papered over. The recognizer is now
//! SenseVoice, run on one VAD-segmented utterance at a time, and the vocabulary is still
//! just phrases: everything here is testable without a microphone and that is the point.
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

/// The names a person calls the duck by. Ordered longest first so 小鸭子 is not read as 小鸭
/// plus a stray 子.
///
/// A sentence that contains one of these is a **remark**, not a command: the daemon answers
/// it (`brain`) and speaks the answer (`speak`), and the rest of the sentence is what it was
/// said to. The wake word exists because the alternative — answer everything the microphone
/// segments — would make the duck talk back to the television six times an evening.
/// The homophones are there because the recognizer writes what it hears: a quiet "小鸭，你好吗"
/// came back as "哇我好吗", and a "小鸭子" as "小呀小鸭". 小鸭 and 鸭子 are the two that matter;
/// the rest are cheap insurance.
pub const WAKE: &[&str] = &[
    "小鸭子", "小鸭", "小呀", "小亚", "小丫", "小牙", "小雅", "鸭鸭", "鸭子",
];

/// The part of the sentence addressed to the duck, or `None` if it was not addressed to it.
///
/// An empty string means the person only said the name ("小鸭？"): the caller's cue to answer
/// without asking a model anything.
pub fn remark(text: &str) -> Option<String> {
    let normalized = normalize(text);
    // Several names can match the same sentence — "小呀小鸭" holds both 小呀 and 小鸭 — so
    // take the one that leaves the least behind: the recognizer puts the name where the
    // person put it, and a name at the end of the sentence should not become a remark.
    WAKE.iter()
        .filter_map(|name| normalized.split_once(name))
        .map(|(_, rest)| rest.to_owned())
        .min_by_key(|rest| rest.chars().count())
}

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
    fn the_name_splits_a_remark_from_the_service() {
        assert_eq!(remark("小鸭，你好吗").as_deref(), Some("你好吗"));
        assert_eq!(remark("小鸭子你在干什么").as_deref(), Some("你在干什么"));
        assert_eq!(remark("你好呀鸭子").as_deref(), Some(""));
        // The name alone is a greeting, not a question. What the recognizer actually
        // returns for a quiet "小鸭子，你在干什么" is 小呀小鸭 — both names match, and the
        // answer is the match that leaves nothing behind.
        assert_eq!(remark("小鸭？").as_deref(), Some(""));
        assert_eq!(remark("小呀小鸭。").as_deref(), Some(""));
        assert_eq!(remark("小亚你好").as_deref(), Some("你好"));
        assert_eq!(remark("今天天气不错"), None);
    }

    #[test]
    fn a_command_with_the_name_in_front_is_still_a_command() {
        // Commands win: 1.5 s of local matching against 2 s of network and inference, and
        // "小鸭坐下" is not a request for an opinion about sitting.
        assert_eq!(parse("小鸭坐下"), Some(Command::SitToggle));
        assert_eq!(remark("小鸭坐下").as_deref(), Some("坐下"));
    }
}
