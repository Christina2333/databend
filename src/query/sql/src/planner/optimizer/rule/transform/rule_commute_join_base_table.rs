// Copyright 2023 Datafuse Labs.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use common_exception::Result;

use crate::optimizer::rule::Rule;
use crate::optimizer::rule::TransformResult;
use crate::optimizer::RuleID;
use crate::optimizer::SExpr;
use crate::plans::Join;
use crate::plans::JoinType;
use crate::plans::Operator;
use crate::plans::PatternPlan;
use crate::plans::RelOp;

/// Rule to apply commutativity of join operator.
/// In opposite to RuleCommuteJoin, this rule only applies to base tables.
pub struct RuleCommuteJoinBaseTable {
    id: RuleID,
    patterns: Vec<SExpr>,
}

impl RuleCommuteJoinBaseTable {
    pub fn new() -> Self {
        Self {
            id: RuleID::CommuteJoinBaseTable,

            // LogicalJoin
            // | \
            // *  *
            patterns: vec![SExpr::create_binary(
                PatternPlan {
                    plan_type: RelOp::Join,
                }
                .into(),
                SExpr::create_pattern_leaf(),
                SExpr::create_pattern_leaf(),
            )],
        }
    }
}

impl Rule for RuleCommuteJoinBaseTable {
    fn id(&self) -> RuleID {
        self.id
    }

    fn apply(&self, s_expr: &SExpr, state: &mut TransformResult) -> Result<()> {
        let mut join: Join = s_expr.plan().clone().try_into()?;
        let left_child = s_expr.child(0)?;
        let right_child = s_expr.child(1)?;

        // Skip if the children are not base tables.
        if left_child.plan.rel_op() == RelOp::Join || right_child.plan.rel_op() == RelOp::Join {
            return Ok(());
        }

        match join.join_type {
            JoinType::Inner
            | JoinType::Cross
            | JoinType::Left
            | JoinType::Right
            | JoinType::LeftSemi
            | JoinType::RightSemi
            | JoinType::LeftAnti
            | JoinType::LeftMark
            | JoinType::RightAnti => {
                // Swap the join conditions side
                (join.left_conditions, join.right_conditions) =
                    (join.right_conditions, join.left_conditions);
                join.join_type = join.join_type.opposite();
                let mut result =
                    SExpr::create_binary(join.into(), right_child.clone(), left_child.clone());

                // Disable the following rules for the generated expression
                result.set_applied_rule(&RuleID::CommuteJoin);
                result.set_applied_rule(&RuleID::CommuteJoinBaseTable);
                result.set_applied_rule(&RuleID::LeftAssociateJoin);
                result.set_applied_rule(&RuleID::LeftExchangeJoin);
                result.set_applied_rule(&RuleID::RightAssociateJoin);
                result.set_applied_rule(&RuleID::RightExchangeJoin);
                result.set_applied_rule(&RuleID::ExchangeJoin);

                state.add_result(result);
            }
            _ => {}
        }

        Ok(())
    }

    fn patterns(&self) -> &Vec<SExpr> {
        &self.patterns
    }

    fn transformation(&self) -> bool {
        false
    }
}
