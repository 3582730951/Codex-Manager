use serde_json::{json, Map, Value};
use std::io::{BufRead, BufReader, Cursor, Read};
use std::sync::{Arc, Mutex};

use super::*;
use super::super::{
    convert_openai_chat_stream_chunk_with_tool_name_restore_map,
    convert_openai_completions_stream_chunk, ToolNameRestoreMap,
};

#[path = "stream_readers/common.rs"]
mod common;
#[path = "stream_readers/passthrough.rs"]
mod passthrough;
#[path = "stream_readers/openai_completions.rs"]
mod openai_completions;
#[path = "stream_readers/openai_chat.rs"]
mod openai_chat;
#[path = "stream_readers/anthropic.rs"]
mod anthropic;

use common::{collector_output_text_trimmed, mark_collector_terminal_success};
pub(crate) use common::PassthroughSseCollector;
pub(crate) use passthrough::PassthroughSseUsageReader;
pub(crate) use openai_completions::OpenAICompletionsSseReader;
pub(crate) use openai_chat::OpenAIChatCompletionsSseReader;
pub(crate) use anthropic::AnthropicSseReader;