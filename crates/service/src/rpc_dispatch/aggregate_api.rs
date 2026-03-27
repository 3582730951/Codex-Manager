use codexmanager_core::rpc::types::{AggregateApiListResult, JsonRpcRequest, JsonRpcResponse};

use crate::{
    create_aggregate_api, delete_aggregate_api, list_aggregate_apis, read_aggregate_api_secret,
    test_aggregate_api_connection, update_aggregate_api,
};

pub(super) fn try_handle(req: &JsonRpcRequest) -> Option<JsonRpcResponse> {
    let result = match req.method.as_str() {
        "aggregateApi/list" => super::value_or_error(
            list_aggregate_apis().map(|items| AggregateApiListResult { items }),
        ),
        "aggregateApi/create" => {
            let provider_type = super::string_param(req, "providerType");
            let supplier_name = super::string_param(req, "supplierName");
            let sort = super::i64_param(req, "sort");
            let url = super::string_param(req, "url");
            let key = super::string_param(req, "key");
            super::value_or_error(create_aggregate_api(
                url,
                key,
                provider_type,
                supplier_name,
                sort,
            ))
        }
        "aggregateApi/update" => {
            let api_id = super::str_param(req, "id").unwrap_or("");
            let provider_type = super::string_param(req, "providerType");
            let supplier_name = super::string_param(req, "supplierName");
            let sort = super::i64_param(req, "sort");
            let url = super::string_param(req, "url");
            let key = super::string_param(req, "key");
            super::ok_or_error(update_aggregate_api(
                api_id,
                url,
                key,
                provider_type,
                supplier_name,
                sort,
            ))
        }
        "aggregateApi/readSecret" => {
            let api_id = super::str_param(req, "id").unwrap_or("");
            super::value_or_error(read_aggregate_api_secret(api_id))
        }
        "aggregateApi/delete" => {
            let api_id = super::str_param(req, "id").unwrap_or("");
            super::ok_or_error(delete_aggregate_api(api_id))
        }
        "aggregateApi/testConnection" => {
            let api_id = super::str_param(req, "id").unwrap_or("");
            super::value_or_error(test_aggregate_api_connection(api_id))
        }
        _ => return None,
    };

    Some(super::response(req, result))
}
