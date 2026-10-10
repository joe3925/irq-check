use log::*;
// Copyright (c) 2024 <Wei Li>.
//
// This source code is licensed under the GNU license found in the
// LICENSE file in the root directory of this source tree.

use crate::mir::function::FuncId;
use self::strategies::context_strategy::KCallSiteSensitive;
use std::time::Instant;


use crate::graph::pag::*;
use crate::pts_set::points_to::HybridPointsToSet;
use crate::pts_set::pt_data::DiffPTData;

pub mod andersen;
pub mod context_sensitive;
pub mod propagator;
pub mod strategies;

pub type NodeId = PAGNodeId;
pub type EdgeId = PAGEdgeId;
pub type PointsTo<T> = HybridPointsToSet<T>;
pub type DiffPTDataTy = DiffPTData<NodeId, NodeId, PointsTo<NodeId>>;

#[derive(Clone, Copy, Debug)]
pub enum PTAType {
    Andersen,
    CallSiteSensitive
}

pub trait PointerAnalysis<'tcx, 'compilation> {
    fn pre_analysis(&mut self) {}
    // Initialization for the analysis.
    fn initialize(&mut self);
    // Solve the worklist problem.
    fn propagate(&mut self);
    // Finalize the analysis.
    fn finalize(&self);

    fn analyze(&mut self) {
        self.pre_analysis();

        // Main analysis phase
        let now = Instant::now();

        self.initialize();
        self.propagate();
        
        let elapsed = now.elapsed();
        println!("Pointer analysis completed.");
        println!(
            "Analysis time: {}",
            humantime::format_duration(elapsed).to_string()
        );
        self.finalize();
    }
}
