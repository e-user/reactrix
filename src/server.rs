// This file is part of reactrix.
//
// Copyright 2020 Alexander Dorn
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

use super::{Aggregatrix, Api, Results};
use warp::filters::BoxedFilter;
use warp::{Filter, Reply};

pub fn prepare<A: Aggregatrix>(
    state: A::State,
    results: Results<A>,
    api: Api,
) -> BoxedFilter<(impl Reply,)> {
    let schema = A::schema();
    let filter = A::filter(state, results, api);

    let graphql = warp::any()
        .and(warp::path("graphql"))
        .and(juniper_warp::make_graphql_filter(schema, filter.boxed()));

    let graphiql = warp::get2()
        .and(warp::path::end())
        .and(juniper_warp::graphiql_filter("/graphql"));

    graphql
        .or(graphiql)
        .with(warp::log("reactrix::server"))
        .with(
            warp::cors()
                .allow_any_origin()
                .allow_methods(vec!["GET", "POST"])
                .allow_headers(vec!["authorization", "content-type"]),
        )
        .boxed()
}
