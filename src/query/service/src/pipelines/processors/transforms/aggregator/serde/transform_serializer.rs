use std::sync::Arc;

use common_exception::Result;
use common_expression::types::string::StringColumnBuilder;
use common_expression::Column;
use common_expression::DataBlock;
use common_functions::aggregates::StateAddr;
use common_hashtable::HashtableEntryRefLike;
use common_hashtable::HashtableLike;
use common_pipeline_core::processors::port::InputPort;
use common_pipeline_core::processors::port::OutputPort;
use common_pipeline_core::processors::processor::ProcessorPtr;
use common_pipeline_transforms::processors::transforms::BlockMetaTransform;
use common_pipeline_transforms::processors::transforms::BlockMetaTransformer;

use crate::pipelines::processors::transforms::aggregator::aggregate_meta::AggregateMeta;
use crate::pipelines::processors::transforms::aggregator::estimated_key_size;
use crate::pipelines::processors::transforms::aggregator::serde::serde_meta::AggregateSerdeMeta;
use crate::pipelines::processors::transforms::group_by::HashMethodBounds;
use crate::pipelines::processors::transforms::group_by::KeysColumnBuilder;
use crate::pipelines::processors::AggregatorParams;

pub struct TransformGroupBySerializer<Method: HashMethodBounds> {
    method: Method,
}

impl<Method: HashMethodBounds> TransformGroupBySerializer<Method> {
    pub fn try_create(
        input: Arc<InputPort>,
        output: Arc<OutputPort>,
        method: Method,
    ) -> Result<ProcessorPtr> {
        Ok(ProcessorPtr::create(BlockMetaTransformer::create(
            input,
            output,
            TransformGroupBySerializer { method },
        )))
    }
}

impl<Method> BlockMetaTransform<AggregateMeta<Method, ()>> for TransformGroupBySerializer<Method>
where Method: HashMethodBounds
{
    const NAME: &'static str = "TransformGroupBySerializer";

    fn transform(&mut self, meta: AggregateMeta<Method, ()>) -> Result<DataBlock> {
        match meta {
            AggregateMeta::Partitioned { .. } => unreachable!(),
            AggregateMeta::Serialized(_) => unreachable!(),
            AggregateMeta::HashTable(payload) => {
                let value_size = estimated_key_size(&payload.hashtable);
                let keys_len = Method::HashTable::len(&payload.hashtable);
                let mut group_key_builder = self.method.keys_column_builder(keys_len, value_size);

                for group_entity in Method::HashTable::iter(&payload.hashtable) {
                    group_key_builder.append_value(group_entity.key());
                }

                let data_block = DataBlock::new_from_columns(vec![group_key_builder.finish()]);
                data_block.add_meta(Some(AggregateSerdeMeta::create(payload.bucket)))
            }
        }
    }
}

pub struct TransformAggregateSerializer<Method: HashMethodBounds> {
    method: Method,
    params: Arc<AggregatorParams>,
}

impl<Method: HashMethodBounds> TransformAggregateSerializer<Method> {
    pub fn try_create(
        input: Arc<InputPort>,
        output: Arc<OutputPort>,
        method: Method,
        params: Arc<AggregatorParams>,
    ) -> Result<ProcessorPtr> {
        Ok(ProcessorPtr::create(BlockMetaTransformer::create(
            input,
            output,
            TransformAggregateSerializer { method, params },
        )))
    }
}

impl<Method> BlockMetaTransform<AggregateMeta<Method, usize>>
    for TransformAggregateSerializer<Method>
where Method: HashMethodBounds
{
    const NAME: &'static str = "TransformAggregateSerializer";

    fn transform(&mut self, meta: AggregateMeta<Method, usize>) -> Result<DataBlock> {
        match meta {
            AggregateMeta::Partitioned { .. } => unreachable!(),
            AggregateMeta::Serialized(_) => unreachable!(),
            AggregateMeta::HashTable(payload) => {
                let value_size = estimated_key_size(&payload.hashtable);
                let keys_len = Method::HashTable::len(&payload.hashtable);

                let funcs = &self.params.aggregate_functions;
                let offsets_aggregate_states = &self.params.offsets_aggregate_states;

                // Builders.
                let mut state_builders = (0..funcs.len())
                    .map(|_| StringColumnBuilder::with_capacity(keys_len, keys_len * 4))
                    .collect::<Vec<_>>();

                let mut group_key_builder = self.method.keys_column_builder(keys_len, value_size);

                for group_entity in Method::HashTable::iter(&payload.hashtable) {
                    let place = Into::<StateAddr>::into(*group_entity.get());

                    for (idx, func) in funcs.iter().enumerate() {
                        let arg_place = place.next(offsets_aggregate_states[idx]);
                        func.serialize(arg_place, &mut state_builders[idx].data)?;
                        state_builders[idx].commit_row();
                    }

                    group_key_builder.append_value(group_entity.key());
                }

                let mut columns = Vec::with_capacity(state_builders.len() + 1);

                for builder in state_builders.into_iter() {
                    columns.push(Column::String(builder.build()));
                }

                columns.push(group_key_builder.finish());
                let data_block = DataBlock::new_from_columns(columns);
                data_block.add_meta(Some(AggregateSerdeMeta::create(payload.bucket)))
            }
        }
    }
}
