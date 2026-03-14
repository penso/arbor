use graphql_client::GraphQLQuery;

type DateTime = String;
#[allow(clippy::upper_case_acronyms)]
type URI = String;

#[allow(dead_code)]
#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "src/graphql/schema.graphql",
    query_path = "src/graphql/review_threads.graphql",
    response_derives = "Debug, Clone"
)]
pub(crate) struct ReviewThreads;

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "src/graphql/schema.graphql",
    query_path = "src/graphql/pull_request_details.graphql",
    response_derives = "Debug, Clone"
)]
pub(crate) struct PullRequestDetails;
