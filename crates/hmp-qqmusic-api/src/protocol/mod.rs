//! 请求协议层：签名、comm 公共参数、CGI 请求描述符（对应上游
//! `algorithms/`、`core/versioning.py`、`core/request.py`、`core/api_context.py`）。

pub mod cgi;
pub mod comm;
pub mod search;
pub mod sign;
