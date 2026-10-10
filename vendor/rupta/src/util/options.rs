// Copyright (c) 2024 <Wei Li>.
//
// This source code is licensed under the GNU license found in the
// LICENSE file in the root directory of this source tree.

//! Analysis options.

use crate::pta::PTAType;

#[derive(Clone, Debug)]
pub struct AnalysisOptions {
    pub entry_func: String,
    pub entry_def_id: Option<u32>,
    pub pta_type: PTAType,
    // options for context-sensitive analysis
    pub context_depth: u32,
    // options for handling cast propagation
    pub cast_constraint: bool,
    pub stack_filtering: bool,
    pub rceus: bool,

    pub dump_stats: bool,
    pub call_graph_output: Option<String>,
    pub pts_output: Option<String>,
    pub mir_output: Option<String>,
    pub type_indices_output: Option<String>,
    pub dyn_calls_output: Option<String>,
    pub unsafe_stat_output: Option<String>,
    pub func_ctxts_output: Option<String>, 
}

impl Default for AnalysisOptions {
    fn default() -> Self {
        Self {
            entry_func: String::new(),
            entry_def_id: None,
            pta_type: PTAType::CallSiteSensitive,
            context_depth: 1,
            cast_constraint: false,
            stack_filtering: false,
            rceus: false,
            dump_stats: true,
            call_graph_output: None,
            pts_output: None,
            mir_output: None,
            type_indices_output: None,
            dyn_calls_output: None,
            unsafe_stat_output: None,
            func_ctxts_output: None,
        }
    }
}
