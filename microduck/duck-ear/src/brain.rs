//! The conversation half: a transcript that is not a command goes to a model on the LAN.
//!
//! The model runs on the user's workstation (`ollama serve`, see the operations note
//! `docs/语音对话操作.md`), not on the board. The board is a 4×A55 with 2 GB of memory; it
//! can recognize speech and play sound, and it cannot run a model worth talking to.
//!
//! Two settings in the request matter more than the model choice:
//!
//! - `"stream": false` — the daemon cannot speak before the sentence is finished, so there is
//!   nothing to gain from tokens as they arrive, and one blocking request is much less code.
//! - `"think": false` — Qwen3.5 thinks by default, which spends seconds on hidden tokens
//!   that nobody hears. Measured on the host: 1.9 s for a two-sentence answer with it off.
//! - `keep_alive` — Ollama unloads an idle model after five minutes and the next question
//!   then pays a five-to-eight second load. Asking for thirty minutes keeps a spoken
//!   conversation's pauses warm.
//!
//! The system prompt is where the duck's manners live: answers are **read aloud**, so they
//! are short, plain, and in the same Chinese the person is speaking.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::net::Service;

/// One question, one answer. The host measured 1.9 s warm and 8 s cold for ~40 tokens; 30 s
/// is the "the model is not coming" line, not a typical wait.
const TIMEOUT: Duration = Duration::from_secs(30);

/// Long enough for two or three spoken sentences, short enough that the answer arrives while
/// the person is still standing there. The system prompt asks for brevity; this enforces it.
const NUM_PREDICT: u32 = 120;

/// Ollama's default idle unload is five minutes; a conversation has pauses longer than that.
const KEEP_ALIVE: &str = "30m";

/// How many past messages are sent back. Three exchanges: enough for "它是什么颜色的"
/// to resolve 它, short enough that a long session does not grow the prompt without bound.
const HISTORY: usize = 6;

const SYSTEM: &str = "你是一只叫小鸭的中文机器人，身体是一只黄色的鸭子，有一个摄像头、一个麦克风和一个小喇叭。\
你的回答会直接被朗读出来，所以请用口语、简短地回答，最多两句话；不要用 Markdown、列表、表情符号、括号或颜文字。\
你可以被要求做动作：说「坐下」你会坐下或站起来，说「前进」你会向前走一步，说「叫一声」你会叫。可以自然地提到这些。\
你不知道的事情就说不知道，不要编造你看到的画面或听到的声音。";

pub struct Brain {
    service: Service,
    model: String,
    /// Alternating user/assistant messages, oldest first, capped at [`HISTORY`].
    messages: Vec<Value>,
}

impl Brain {
    pub fn new(addr: &str, model: String) -> Result<Self> {
        Ok(Self {
            service: Service::parse(addr).context("the brain's address")?,
            model,
            messages: Vec::new(),
        })
    }

    /// Ask the host to load the model now, in the background.
    ///
    /// The first request after a load takes five to eight seconds; doing it while the daemon
    /// is still initialising the recognizer means the first real question is answered at the
    /// warm speed. Failure is logged and ignored — a host that is down must not keep the ear
    /// from starting, it just means remarks are not answered.
    pub fn warm(&self) {
        let service = self.service.clone();
        let model = self.model.clone();
        std::thread::spawn(move || {
            let body = json!({
                "model": model,
                "messages": [{"role": "user", "content": "在吗"}],
                "stream": false,
                "think": false,
                "keep_alive": KEEP_ALIVE,
                "options": {"num_predict": 1},
            })
            .to_string();
            match service.post_json("/api/chat", &body, TIMEOUT) {
                Ok(_) => tracing::info!(model = %model, "the brain is warm"),
                Err(error) => tracing::warn!(%error, "the brain is not answering yet"),
            }
        });
    }

    /// Send one remark, get one spoken sentence back.
    ///
    /// The user message is rolled back if the request fails, so a host that was down for a
    /// minute does not leave unanswered questions in the history for the model to trip over.
    pub fn reply(&mut self, text: &str) -> Result<String> {
        self.messages.push(json!({"role": "user", "content": text}));
        let answer = self.ask();
        if answer.is_err() {
            self.messages.pop();
        }
        answer
    }

    fn ask(&mut self) -> Result<String> {
        let mut messages = Vec::with_capacity(self.messages.len() + 1);
        messages.push(json!({"role": "system", "content": SYSTEM}));
        messages.extend(self.messages.iter().cloned());

        let body = json!({
            "model": self.model,
            "messages": messages,
            "stream": false,
            "think": false,
            "keep_alive": KEEP_ALIVE,
            "options": {"num_predict": NUM_PREDICT, "temperature": 0.7},
        })
        .to_string();

        let raw = self
            .service
            .post_json("/api/chat", &body, TIMEOUT)
            .context("asking the brain")?;
        let value: Value = serde_json::from_slice(&raw).context("the brain's reply is not JSON")?;
        if let Some(error) = value.get("error").and_then(Value::as_str) {
            bail!("the brain refused: {error}");
        }
        let content = value["message"]["content"]
            .as_str()
            .unwrap_or_default()
            .trim()
            .to_owned();
        if content.is_empty() {
            bail!("the brain answered with nothing");
        }

        self.messages
            .push(json!({"role": "assistant", "content": content}));
        // Drop the oldest exchange, never the newest.
        while self.messages.len() > HISTORY {
            self.messages.remove(0);
        }
        Ok(content)
    }
}
