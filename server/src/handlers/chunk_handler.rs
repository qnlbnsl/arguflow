use super::auth_handler::{AdminOnly, LoggedUser};
use crate::data::models::{
    ChatMessageProxy, ChunkCollection, ChunkCollectionBookmark, ChunkMetadata,
    ChunkMetadataWithFileData, DatasetAndOrgWithSubAndPlan, Pool, ServerDatasetConfiguration,
    StripePlan,
};
use crate::errors::{DefaultError, ServiceError};
use crate::get_env;
use crate::operators::chunk_operator::get_metadata_from_id_query;
use crate::operators::chunk_operator::*;
use crate::operators::collection_operator::{
    create_chunk_bookmark_query, get_collection_by_id_query,
};
use crate::operators::model_operator::create_embedding;
use crate::operators::qdrant_operator::update_qdrant_point_query;
use crate::operators::qdrant_operator::{
    create_new_qdrant_point_query, delete_qdrant_point_id_query, recommend_qdrant_query,
};
use crate::operators::search_operator::{
    global_unfiltered_top_match_query, search_full_text_chunks, search_full_text_collections,
    search_hybrid_chunks, search_semantic_chunks, search_semantic_collections,
};
use actix_web::web::Bytes;
use actix_web::{web, HttpResponse};
use chrono::NaiveDateTime;
use dateparser::DateTimeUtc;
use openai_dive::v1::api::Client;
use openai_dive::v1::resources::chat::{
    ChatCompletionParameters, ChatMessage, ChatMessageContent, Role,
};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::process::Command;
use tokio_stream::StreamExt;
use utoipa::{IntoParams, ToSchema};

pub async fn user_owns_chunk(
    user_id: uuid::Uuid,
    chunk_id: uuid::Uuid,
    dataset_id: uuid::Uuid,
    pool: web::Data<Pool>,
) -> Result<ChunkMetadata, actix_web::Error> {
    let chunks = web::block(move || get_metadata_from_id_query(chunk_id, dataset_id, pool))
        .await?
        .map_err(|err| ServiceError::BadRequest(err.message.into()))?;

    if chunks.author_id != user_id {
        return Err(ServiceError::Forbidden.into());
    }

    Ok(chunks)
}

pub async fn user_owns_chunk_tracking_id(
    user_id: uuid::Uuid,
    tracking_id: String,
    dataset_id: uuid::Uuid,
    pool: web::Data<Pool>,
) -> Result<ChunkMetadata, actix_web::Error> {
    let chunks =
        web::block(move || get_metadata_from_tracking_id_query(tracking_id, dataset_id, pool))
            .await?
            .map_err(|err| ServiceError::BadRequest(err.message.into()))?;

    if chunks.author_id != user_id {
        return Err(ServiceError::Forbidden.into());
    }

    Ok(chunks)
}

#[derive(Serialize, Deserialize, ToSchema, Clone)]
pub struct CreateChunkData {
    /// HTML content of the chunk. This can also be plaintext. The innerText of the HTML will be used to create the embedding vector. The point of using HTML is for convienience, as some users have applications where users submit HTML content.
    pub chunk_html: Option<String>,
    /// Link to the chunk. This can also be any string. Frequently, this is a link to the source of the chunk. The link value will not affect the embedding creation.
    pub link: Option<String>,
    /// Tag set is a comma separated list of tags. This can be used to filter chunks by tag. Unlike with metadata filtering, HNSW indices will exist for each tag such that there is not a performance hit for filtering on them.
    pub tag_set: Option<String>,
    /// File_uuid is the uuid of the file that the chunk is associated with. This is used to associate chunks with files. This is useful for when you want to delete a file and all of its associated chunks.
    pub file_uuid: Option<uuid::Uuid>,
    /// Metadata is a JSON object which can be used to filter chunks. This is useful for when you want to filter chunks by arbitrary metadata. Unlike with tag filtering, there is a performance hit for filtering on metadata.
    pub metadata: Option<serde_json::Value>,
    /// Chunk_vector is a vector of floats which can be used instead of generating a new embedding. This is useful for when you are using a pre-embedded dataset. If this is not provided, the innerText of the chunk_html will be used to create the embedding.
    pub chunk_vector: Option<Vec<f32>>,
    /// Tracking_id is a string which can be used to identify a chunk. This is useful for when you are coordinating with an external system and want to use the tracking_id to identify the chunk.
    pub tracking_id: Option<String>,
    /// Collection_id is the id of the collection that the chunk should be placed into. This is useful for when you want to create a chunk and add it to a collection in one request.
    pub collection_id: Option<uuid::Uuid>,
    /// Time_stamp should be an ISO 8601 combined date and time without timezone. It is used for time window filtering and recency-biasing search results.
    pub time_stamp: Option<String>,
    /// Weight is a float which can be used to bias search results. This is useful for when you want to bias search results for a chunk. The magnitude only matters relative to other chunks in the chunk's dataset dataset.
    pub weight: Option<f64>,
}

pub fn convert_html(html: &str) -> Result<String, DefaultError> {
    let html_parse_result = Command::new("./server-python/html-converter.py")
        .arg(html)
        .output();

    let content = match html_parse_result {
        Ok(result) => {
            if result.status.success() {
                Some(
                    String::from_utf8(result.stdout)
                        .unwrap()
                        .lines()
                        .collect::<Vec<&str>>()
                        .join(" ")
                        .trim_end()
                        .to_string(),
                )
            } else {
                return Err(DefaultError {
                    message: "Could not parse html",
                });
            }
        }
        Err(_) => {
            return Err(DefaultError {
                message: "Could not parse html",
            });
        }
    };

    match content {
        Some(content) => Ok(content),
        None => Err(DefaultError {
            message: "Could not parse html",
        }),
    }
}
#[derive(Serialize, Deserialize, Clone, ToSchema)]
pub struct ReturnCreatedChunk {
    pub chunk_metadata: ChunkMetadata,
    pub duplicate: bool,
}

/// create_chunk
///
/// Create a new chunk. If the chunk has the same tracking_id as an existing chunk, the request will fail. Once a chunk is created, it can be searched for using the search endpoint.
#[utoipa::path(
    post,
    path = "/chunk",
    context_path = "/api",
    tag = "chunk",
    request_body(content = CreateChunkData, description = "JSON request payload to create a new chunk (chunk)", content_type = "application/json"),
    responses(
        (status = 200, description = "JSON response payload containing the created chunk", body = ReturnCreatedChunk),
        (status = 400, description = "Service error relating to to creating a chunk, likely due to conflicting tracking_id", body = DefaultError),
    )
)]
pub async fn create_chunk(
    chunk: web::Json<CreateChunkData>,
    pool: web::Data<Pool>,
    user: AdminOnly,
    dataset_org_plan_sub: DatasetAndOrgWithSubAndPlan,
) -> Result<HttpResponse, actix_web::Error> {
    let pool1 = pool.clone();
    let pool2 = pool.clone();
    let pool3 = pool.clone();
    let count_pool = pool.clone();
    let count_dataset_id = dataset_org_plan_sub.dataset.id;

    let chunk_count =
        web::block(move || get_row_count_for_dataset_id_query(count_dataset_id, count_pool))
            .await?
            .map_err(|err| ServiceError::BadRequest(err.message.into()))?;

    if chunk_count
        >= dataset_org_plan_sub
            .organization
            .plan
            .unwrap_or(StripePlan::default())
            .chunk_count
    {
        return Ok(HttpResponse::UpgradeRequired()
            .json(json!({"message": "Must upgrade your plan to add more chunks"})));
    }

    let chunk_tracking_id = chunk
        .tracking_id
        .clone()
        .filter(|chunk_tracking| !chunk_tracking.is_empty());
    let chunk_collection_id = chunk.collection_id;

    let mut collision: Option<uuid::Uuid> = None;

    let content =
        convert_html(chunk.chunk_html.as_ref().unwrap_or(&"".to_string())).map_err(|err| {
            ServiceError::BadRequest(format!("Could not parse html: {}", err.message))
        })?;
    let dataset_config =
        ServerDatasetConfiguration::from_json(dataset_org_plan_sub.dataset.server_configuration);
    let embedding_vector = if let Some(embedding_vector) = chunk.chunk_vector.clone() {
        embedding_vector
    } else {
        create_embedding(&content, dataset_config.clone()).await?
    };

    let first_semantic_result = global_unfiltered_top_match_query(
        embedding_vector.clone(),
        dataset_org_plan_sub.dataset.id,
    )
    .await
    .map_err(|err| {
        ServiceError::BadRequest(format!(
            "Could not get semantic similarity for collision check: {}",
            err.message
        ))
    })?;

    let duplicate_distance_threshold = dataset_config.DUPLICATE_DISTANCE_THRESHOLD.unwrap_or(0.95);

    if first_semantic_result.score >= duplicate_distance_threshold {
        //Sets collision to collided chunk id
        collision = Some(first_semantic_result.point_id);

        let score_chunk_result = web::block(move || {
            get_metadata_from_point_ids(vec![first_semantic_result.point_id], pool2)
        })
        .await?;

        match score_chunk_result {
            Ok(chunk_results) => {
                if chunk_results.is_empty() {
                    delete_qdrant_point_id_query(
                        first_semantic_result.point_id,
                        dataset_org_plan_sub.dataset.id,
                    )
                    .await
                    .map_err(|_| {
                        ServiceError::BadRequest(
                            "Could not delete qdrant point id. Please try again.".into(),
                        )
                    })?;

                    return Err(ServiceError::BadRequest(
                        "There was a data inconsistency issue. Please try again.".into(),
                    )
                    .into());
                }
                chunk_results.first().unwrap().clone()
            }
            Err(err) => {
                return Err(ServiceError::BadRequest(err.message.into()).into());
            }
        };
    }

    let mut chunk_metadata: ChunkMetadata;
    let mut duplicate: bool = false;

    //if collision is not nil, insert chunk with collision
    if collision.is_some() {
        update_qdrant_point_query(
            None,
            collision.expect("Collision must be some"),
            Some(user.0.id),
            None,
            dataset_org_plan_sub.dataset.id,
        )
        .await?;

        chunk_metadata = ChunkMetadata::from_details(
            &content,
            &chunk.chunk_html,
            &chunk.link,
            &chunk.tag_set,
            user.0.id,
            None,
            chunk.metadata.clone(),
            chunk_tracking_id,
            chunk
                .time_stamp
                .clone()
                .map(|ts| -> Result<NaiveDateTime, ServiceError> {
                    //TODO: change all ts parsing to this crate
                    Ok(ts
                        .parse::<DateTimeUtc>()
                        .map_err(|_| {
                            ServiceError::BadRequest("Invalid timestamp format".to_string())
                        })?
                        .0
                        .with_timezone(&chrono::Local)
                        .naive_local())
                })
                .transpose()?,
            dataset_org_plan_sub.dataset.id,
            0.0,
        );
        chunk_metadata = web::block(move || {
            insert_duplicate_chunk_metadata_query(
                chunk_metadata,
                collision.expect("Collision should must be some"),
                chunk.file_uuid,
                pool1,
            )
        })
        .await?
        .map_err(|err| ServiceError::BadRequest(err.message.into()))?;

        duplicate = true;
    }
    //if collision is nil and embedding vector is some, insert chunk with no collision
    else {
        let qdrant_point_id = uuid::Uuid::new_v4();

        chunk_metadata = ChunkMetadata::from_details(
            &content,
            &chunk.chunk_html,
            &chunk.link,
            &chunk.tag_set,
            user.0.id,
            Some(qdrant_point_id),
            chunk.metadata.clone(),
            chunk_tracking_id,
            chunk
                .time_stamp
                .clone()
                .map(|ts| -> Result<NaiveDateTime, ServiceError> {
                    Ok(ts
                        .parse::<DateTimeUtc>()
                        .map_err(|_| {
                            ServiceError::BadRequest("Invalid timestamp format".to_string())
                        })?
                        .0
                        .with_timezone(&chrono::Local)
                        .naive_local())
                })
                .transpose()?,
            dataset_org_plan_sub.dataset.id,
            0.0,
        );

        chunk_metadata = insert_chunk_metadata_query(chunk_metadata, chunk.file_uuid, pool1)
            .await
            .map_err(|err| ServiceError::BadRequest(err.message.into()))?;

        create_new_qdrant_point_query(
            qdrant_point_id,
            embedding_vector,
            chunk_metadata.clone(),
            Some(user.0.id),
            dataset_org_plan_sub.dataset.id,
        )
        .await?;
    }

    if let Some(collection_id_to_bookmark) = chunk_collection_id {
        let chunk_collection_bookmark =
            ChunkCollectionBookmark::from_details(collection_id_to_bookmark, chunk_metadata.id);

        let _ = web::block(move || create_chunk_bookmark_query(pool3, chunk_collection_bookmark))
            .await?;
    }

    Ok(HttpResponse::Ok().json(ReturnCreatedChunk {
        chunk_metadata,
        duplicate,
    }))
}

/// delete_chunk
///
/// Delete a chunk by its id. If deleting a root chunk which has a collision, the most recently created collision will become a new root chunk.
#[utoipa::path(
    delete,
    path = "/chunk/{chunk_id}",
    context_path = "/api",
    tag = "chunk",
    responses(
        (status = 204, description = "Confirmation that the chunk with the id specified was deleted"),
        (status = 400, description = "Service error relating to finding a chunk by tracking_id", body = DefaultError),
    ),
    params(
        ("chunk_id" = Option<uuid>, Path, description = "id of the chunk you want to delete")
    ),
)]
pub async fn delete_chunk(
    chunk_id: web::Path<uuid::Uuid>,
    pool: web::Data<Pool>,
    user: AdminOnly,
    dataset_org_plan_sub: DatasetAndOrgWithSubAndPlan,
) -> Result<HttpResponse, actix_web::Error> {
    let chunk_id_inner = chunk_id.into_inner();
    let pool1 = pool.clone();
    let dataset_id = dataset_org_plan_sub.dataset.id;
    let chunk_metadata = user_owns_chunk(user.0.id, chunk_id_inner, dataset_id, pool).await?;
    let qdrant_point_id = chunk_metadata.qdrant_point_id;

    delete_chunk_metadata_query(
        chunk_id_inner,
        qdrant_point_id,
        dataset_org_plan_sub.dataset,
        pool1,
    )
    .await
    .map_err(|err| ServiceError::BadRequest(err.message.into()))?;

    Ok(HttpResponse::NoContent().finish())
}

/// delete_chunk_by_tracking_id
///
/// Delete a chunk by tracking_id. This is useful for when you are coordinating with an external system and want to use the tracking_id to identify the chunk. If deleting a root chunk which has a collision, the most recently created collision will become a new root chunk.
#[utoipa::path(
    delete,
    path = "/chunk/tracking_id/{tracking_id}",
    context_path = "/api",
    tag = "chunk",
    responses(
        (status = 204, description = "Confirmation that the chunk with the tracking_id specified was deleted"),
        (status = 400, description = "Service error relating to finding a chunk by tracking_id", body = DefaultError),
    ),
    params(
        ("tracking_id" = Option<String>, Path, description = "tracking_id of the chunk you want to delete")
    ),
)]
pub async fn delete_chunk_by_tracking_id(
    tracking_id: web::Path<String>,
    pool: web::Data<Pool>,
    user: AdminOnly,
    dataset_org_plan_sub: DatasetAndOrgWithSubAndPlan,
) -> Result<HttpResponse, actix_web::Error> {
    let tracking_id_inner = tracking_id.into_inner();
    let pool1 = pool.clone();
    let dataset_id = dataset_org_plan_sub.dataset.id;

    let chunk_metadata =
        user_owns_chunk_tracking_id(user.0.id, tracking_id_inner, dataset_id, pool).await?;

    let qdrant_point_id = chunk_metadata.qdrant_point_id;

    delete_chunk_metadata_query(
        chunk_metadata.id,
        qdrant_point_id,
        dataset_org_plan_sub.dataset,
        pool1,
    )
    .await
    .map_err(|err| ServiceError::BadRequest(err.message.into()))?;

    Ok(HttpResponse::NoContent().finish())
}

#[derive(Serialize, Deserialize, Clone, ToSchema)]
pub struct UpdateChunkData {
    /// Id of the chunk you want to update.
    chunk_uuid: uuid::Uuid,
    /// Link of the chunk you want to update. This can also be any string. Frequently, this is a link to the source of the chunk. The link value will not affect the embedding creation. If no link is provided, the existing link will be used.
    link: Option<String>,
    /// HTML content of the chunk you want to update. This can also be plaintext. The innerText of the HTML will be used to create the embedding vector. The point of using HTML is for convienience, as some users have applications where users submit HTML content. If no chunk_html is provided, the existing chunk_html will be used.
    chunk_html: Option<String>,
    /// The metadata is a JSON object which can be used to filter chunks. This is useful for when you want to filter chunks by arbitrary metadata. Unlike with tag filtering, there is a performance hit for filtering on metadata. If no metadata is provided, the existing metadata will be used.
    metadata: Option<serde_json::Value>,
    /// Tracking_id is a string which can be used to identify a chunk. This is useful for when you are coordinating with an external system and want to use the tracking_id to identify the chunk. If no tracking_id is provided, the existing tracking_id will be used.
    tracking_id: Option<String>,
    /// Time_stamp should be an ISO 8601 combined date and time without timezone. It is used for time window filtering and recency-biasing search results. If no time_stamp is provided, the existing time_stamp will be used.
    time_stamp: Option<String>,
    /// Weight is a float which can be used to bias search results. This is useful for when you want to bias search results for a chunk. The magnitude only matters relative to other chunks in the chunk's dataset dataset. If no weight is provided, the existing weight will be used.
    weight: Option<f64>,
}
#[derive(Serialize, Deserialize, Clone, ToSchema)]
pub struct ChunkHtmlUpdateError {
    pub message: String,
    changed_content: String,
}

/// update_chunk
///
/// Update a chunk. If you try to change the tracking_id of the chunk to have the same tracking_id as an existing chunk, the request will fail.
#[utoipa::path(
    put,
    path = "/chunk/update",
    context_path = "/api",
    tag = "chunk",
    request_body(content = UpdateChunkData, description = "JSON request payload to update a chunk (chunk)", content_type = "application/json"),
    responses(
        (status = 204, description = "No content Ok response indicating the chunk was updated as requested",),
        (status = 400, description = "Service error relating to to updating chunk, likely due to conflicting tracking_id", body = DefaultError),
    )
)]
pub async fn update_chunk(
    chunk: web::Json<UpdateChunkData>,
    pool: web::Data<Pool>,
    user: AdminOnly,
    dataset_org_plan_sub: DatasetAndOrgWithSubAndPlan,
) -> Result<HttpResponse, actix_web::Error> {
    let pool1 = pool.clone();
    let pool2 = pool.clone();
    let dataset_id = dataset_org_plan_sub.dataset.id;
    let chunk_metadata = user_owns_chunk(user.0.id, chunk.chunk_uuid, dataset_id, pool).await?;

    let link = chunk
        .link
        .clone()
        .unwrap_or_else(|| chunk_metadata.link.clone().unwrap_or_default());
    let chunk_tracking_id = chunk
        .tracking_id
        .clone()
        .filter(|chunk_tracking| !chunk_tracking.is_empty());

    let new_content = convert_html(chunk.chunk_html.as_ref().unwrap_or(&chunk_metadata.content))
        .map_err(|err| {
            ServiceError::BadRequest(format!("Could not parse html: {}", err.message))
        })?;

    let embedding_vector = create_embedding(
        &new_content,
        ServerDatasetConfiguration::from_json(dataset_org_plan_sub.dataset.server_configuration),
    )
    .await?;

    let chunk_html = match chunk.chunk_html.clone() {
        Some(chunk_html) => Some(chunk_html),
        None => chunk_metadata.chunk_html,
    };

    let chunk_id1 = chunk.chunk_uuid;
    let qdrant_point_id = web::block(move || get_qdrant_id_from_chunk_id_query(chunk_id1, pool1))
        .await?
        .map_err(|_| ServiceError::BadRequest("chunk not found".into()))?;

    let metadata = ChunkMetadata::from_details_with_id(
        chunk.chunk_uuid,
        &new_content,
        &chunk_html,
        &Some(link),
        &chunk_metadata.tag_set,
        user.0.id,
        chunk_metadata.qdrant_point_id,
        <std::option::Option<serde_json::Value> as Clone>::clone(&chunk.metadata)
            .or(chunk_metadata.metadata),
        chunk_tracking_id,
        chunk
            .time_stamp
            .clone()
            .map(|ts| -> Result<NaiveDateTime, ServiceError> {
                //TODO: change all ts parsing to this crate
                Ok(ts
                    .parse::<DateTimeUtc>()
                    .map_err(|_| ServiceError::BadRequest("Invalid timestamp format".to_string()))?
                    .0
                    .with_timezone(&chrono::Local)
                    .naive_local())
            })
            .transpose()?
            .or(chunk_metadata.time_stamp),
        dataset_id,
        chunk.weight.unwrap_or(1.0),
    );
    let metadata1 = metadata.clone();
    update_chunk_metadata_query(metadata, None, dataset_id, pool2)
        .await
        .map_err(|err| ServiceError::BadRequest(err.message.into()))?;

    update_qdrant_point_query(
        // If the chunk is a collision, we don't want to update the qdrant point
        if chunk_metadata.qdrant_point_id.is_none() {
            None
        } else {
            Some(metadata1)
        },
        qdrant_point_id,
        Some(user.0.id),
        Some(embedding_vector),
        dataset_id,
    )
    .await?;

    Ok(HttpResponse::NoContent().finish())
}

#[derive(Serialize, Deserialize, Clone, ToSchema)]
pub struct UpdateChunkByTrackingIdData {
    /// Tracking_id of the chunk you want to update. This is required to match an existing chunk.
    tracking_id: String,
    /// Link of the chunk you want to update. This can also be any string. Frequently, this is a link to the source of the chunk. The link value will not affect the embedding creation. If no link is provided, the existing link will be used.
    link: Option<String>,
    /// HTML content of the chunk you want to update. This can also be plaintext. The innerText of the HTML will be used to create the embedding vector. The point of using HTML is for convienience, as some users have applications where users submit HTML content. If no chunk_html is provided, the existing chunk_html will be used.
    chunk_html: Option<String>,
    /// The metadata is a JSON object which can be used to filter chunks. This is useful for when you want to filter chunks by arbitrary metadata. Unlike with tag filtering, there is a performance hit for filtering on metadata. If no metadata is provided, the existing metadata will be used.
    metadata: Option<serde_json::Value>,
    /// Time_stamp should be an ISO 8601 combined date and time without timezone. It is used for time window filtering and recency-biasing search results. If no time_stamp is provided, the existing time_stamp will be used.
    time_stamp: Option<String>,
    /// Weight is a float which can be used to bias search results. This is useful for when you want to bias search results for a chunk. The magnitude only matters relative to other chunks in the chunk's dataset dataset. If no weight is provided, the existing weight will be used.
    weight: Option<f64>,
}

/// update_chunk_by_tracking_id
///
/// Update a chunk by tracking_id. This is useful for when you are coordinating with an external system and want to use the tracking_id to identify the chunk.
#[utoipa::path(
    put,
    path = "/chunk/tracking_id/update",
    context_path = "/api",
    tag = "chunk",
    request_body(content = UpdateChunkByTrackingIdData, description = "JSON request payload to update a chunk by tracking_id (chunks)", content_type = "application/json"),
    responses(
        (status = 204, description = "Confirmation that the chunk has been updated as per your request",),
        (status = 400, description = "Service error relating to to updating chunk", body = DefaultError),
    ),
)]
pub async fn update_chunk_by_tracking_id(
    chunk: web::Json<UpdateChunkByTrackingIdData>,
    pool: web::Data<Pool>,
    user: AdminOnly,
    dataset_org_plan_sub: DatasetAndOrgWithSubAndPlan,
) -> Result<HttpResponse, actix_web::Error> {
    if chunk.tracking_id.is_empty() {
        return Err(ServiceError::BadRequest(
            "Tracking id must be provided to update by tracking_id".into(),
        )
        .into());
    }
    let tracking_id = chunk.tracking_id.clone();
    let tracking_id1 = tracking_id.clone();

    let pool1 = pool.clone();
    let pool2 = pool.clone();
    let chunk_metadata = user_owns_chunk_tracking_id(
        user.0.id,
        tracking_id,
        dataset_org_plan_sub.dataset.id,
        pool,
    )
    .await?;

    let link = chunk
        .link
        .clone()
        .unwrap_or_else(|| chunk_metadata.link.clone().unwrap_or_default());

    let new_content = convert_html(chunk.chunk_html.as_ref().unwrap_or(&chunk_metadata.content))
        .map_err(|err| {
            ServiceError::BadRequest(format!("Could not parse html: {}", err.message))
        })?;

    let embedding_vector = create_embedding(
        &new_content,
        ServerDatasetConfiguration::from_json(dataset_org_plan_sub.dataset.server_configuration),
    )
    .await?;

    let chunk_html = match chunk.chunk_html.clone() {
        Some(chunk_html) => Some(chunk_html),
        None => chunk_metadata.chunk_html,
    };

    let chunk_id1 = chunk_metadata.id;
    let qdrant_point_id = web::block(move || get_qdrant_id_from_chunk_id_query(chunk_id1, pool1))
        .await?
        .map_err(|_| ServiceError::BadRequest("chunk not found".into()))?;

    let metadata = ChunkMetadata::from_details_with_id(
        chunk_metadata.id,
        &new_content,
        &chunk_html,
        &Some(link),
        &chunk_metadata.tag_set,
        user.0.id,
        chunk_metadata.qdrant_point_id,
        <std::option::Option<serde_json::Value> as Clone>::clone(&chunk.metadata)
            .or(chunk_metadata.metadata),
        Some(tracking_id1),
        chunk
            .time_stamp
            .clone()
            .map(|ts| -> Result<NaiveDateTime, ServiceError> {
                //TODO: change all ts parsing to this crate
                Ok(ts
                    .parse::<DateTimeUtc>()
                    .map_err(|_| ServiceError::BadRequest("Invalid timestamp format".to_string()))?
                    .0
                    .with_timezone(&chrono::Local)
                    .naive_local())
            })
            .transpose()?
            .or(chunk_metadata.time_stamp),
        dataset_org_plan_sub.dataset.id,
        chunk.weight.unwrap_or(1.0),
    );
    let metadata1 = metadata.clone();
    update_chunk_metadata_query(metadata, None, dataset_org_plan_sub.dataset.id, pool2)
        .await
        .map_err(|err| ServiceError::BadRequest(err.message.into()))?;

    update_qdrant_point_query(
        // If the chunk is a collision, we don't want to update the qdrant point
        if chunk_metadata.qdrant_point_id.is_none() {
            None
        } else {
            Some(metadata1)
        },
        qdrant_point_id,
        Some(user.0.id),
        Some(embedding_vector),
        dataset_org_plan_sub.dataset.id,
    )
    .await?;

    Ok(HttpResponse::NoContent().finish())
}

#[derive(Serialize, Deserialize, Clone, ToSchema)]
pub struct SearchChunkData {
    /// Can be either "semantic", "fulltext", or "hybrid". "hybrid" will pull in one page (10 chunks) of both semantic and full-text results then re-rank them using reciprocal rank fusion using the specified weights or BAAI/bge-reranker-large. "semantic" will pull in one page (10 chunks) of the nearest cosine distant vectors. "fulltext" will pull in one page (10 chunks) of full-text results based on SPLADE.
    pub search_type: String,
    /// Query is the search query. This can be any string. The query will be used to create an embedding vector and/or SPLADE vector which will be used to find the result set.
    pub query: String,
    /// Page of chunks to fetch. Each page is 10 chunks. Support for custom page size is coming soon.
    pub page: Option<u64>,
    /// Link set is a comma separated list of links. This can be used to filter chunks by link. HNSW indices do not exist for links, so there is a performance hit for filtering on them.
    pub link: Option<Vec<String>>,
    /// Tag_set is a comma separated list of tags. This can be used to filter chunks by tag. Unlike with metadata filtering, HNSW indices will exist for each tag such that there is not a performance hit for filtering on them.
    pub tag_set: Option<Vec<String>>,
    /// Time_range is a tuple of two ISO 8601 combined date and time without timezone. The first value is the start of the time range and the second value is the end of the time range. This can be used to filter chunks by time range. HNSW indices do not exist for time range, so there is a performance hit for filtering on them.
    pub time_range: Option<(String, String)>,
    /// Filters is a JSON object which can be used to filter chunks. The values on each key in the object will be used to check for an exact substring match on the metadata values for each existing chunk. This is useful for when you want to filter chunks by arbitrary metadata. Unlike with tag filtering, there is a performance hit for filtering on metadata.
    pub filters: Option<serde_json::Value>,
    /// Set date_bias to true to bias search results towards more recent chunks. This will work best in hybrid search mode.
    pub date_bias: Option<bool>,
    /// Set cross_encoder to true to use the BAAI/bge-reranker-large model to re-rank search results. This will only apply if in hybrid search mode. If no weighs are specified, the re-ranker will be used by default.
    pub cross_encoder: Option<bool>,
    /// Weights are a tuple of two floats. The first value is the weight for the semantic search results and the second value is the weight for the full-text search results. This can be used to bias search results towards semantic or full-text results. This will only apply if in hybrid search mode and cross_encoder is set to false.
    pub weights: Option<(f64, f64)>,
}

#[derive(Serialize, Deserialize, Debug, ToSchema, Clone)]
pub struct ScoreChunkDTO {
    pub metadata: Vec<ChunkMetadataWithFileData>,
    pub score: f64,
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct SearchChunkQueryResponseBody {
    pub score_chunks: Vec<ScoreChunkDTO>,
    pub total_chunk_pages: i64,
}

#[derive(Clone)]
pub struct ParsedQuery {
    pub query: String,
    pub quote_words: Option<Vec<String>>,
    pub negated_words: Option<Vec<String>>,
}
fn parse_query(query: String) -> ParsedQuery {
    let re = Regex::new(r#""(.*?)""#).unwrap();
    let quote_words: Vec<String> = re
        .captures_iter(&query.replace('\\', ""))
        .map(|capture| capture[1].to_string())
        .filter(|word| !word.is_empty())
        .collect::<Vec<String>>();

    let quote_words = if quote_words.is_empty() {
        None
    } else {
        Some(quote_words)
    };

    let negated_words: Vec<String> = query
        .split_whitespace()
        .filter(|word| word.starts_with('-'))
        .map(|word| word.strip_prefix('-').unwrap().to_string())
        .collect::<Vec<String>>();

    let negated_words = if negated_words.is_empty() {
        None
    } else {
        Some(negated_words)
    };

    ParsedQuery {
        query,
        quote_words,
        negated_words,
    }
}

/// search
///
/// This route provides the primary search functionality for the API. It can be used to search for chunks by semantic similarity, full-text similarity, or a combination of both. Results' `chunk_html` values will be modified with `<b>` tags for sub-sentence highlighting.
#[utoipa::path(
    post,
    path = "/chunk/search",
    context_path = "/api",
    tag = "chunk",
    request_body(content = SearchChunkData, description = "JSON request payload to semantically search for chunks (chunks)", content_type = "application/json"),
    responses(
        (status = 200, description = "chunks which are similar to the embedding vector of the search query", body = SearchChunkQueryResponseBody),
        (status = 400, description = "Service error relating to searching", body = DefaultError),
    ),
)]
#[allow(clippy::too_many_arguments)]
pub async fn search_chunk(
    data: web::Json<SearchChunkData>,
    _user: LoggedUser,
    pool: web::Data<Pool>,
    dataset_org_plan_sub: DatasetAndOrgWithSubAndPlan,
) -> Result<HttpResponse, actix_web::Error> {
    let page = data.page.unwrap_or(1);
    let dataset_id = dataset_org_plan_sub.dataset.id;
    let parsed_query = parse_query(data.query.clone());

    let result_chunks = match data.search_type.as_str() {
        "fulltext" => search_full_text_chunks(data, parsed_query, page, pool, dataset_id).await?,
        "hybrid" => {
            search_hybrid_chunks(data, parsed_query, page, pool, dataset_org_plan_sub.dataset)
                .await?
        }
        _ => {
            search_semantic_chunks(data, parsed_query, page, pool, dataset_org_plan_sub.dataset)
                .await?
        }
    };

    Ok(HttpResponse::Ok().json(result_chunks))
}

#[derive(Serialize, Deserialize, Clone, ToSchema, IntoParams)]
#[into_params(style = Form, parameter_in = Query)]
pub struct SearchCollectionsData {
    /// The query is the search query. This can be any string. The query will be used to create an embedding vector and/or SPLADE vector which will be used to find the result set.
    pub query: String,
    /// The page of chunks to fetch. Each page is 10 chunks. Support for custom page size is coming soon.
    pub page: Option<u64>,
    /// The link set is a comma separated list of links. This can be used to filter chunks by link. HNSW indices do not exist for links, so there is a performance hit for filtering on them.
    pub link: Option<Vec<String>>,
    /// The tag set is a comma separated list of tags. This can be used to filter chunks by tag. Unlike with metadata filtering, HNSW indices will exist for each tag such that there is not a performance hit for filtering on them.
    pub tag_set: Option<Vec<String>>,
    /// Filters is a JSON object which can be used to filter chunks. The values on each key in the object will be used to check for an exact substring match on the metadata values for each existing chunk. This is useful for when you want to filter chunks by arbitrary metadata. Unlike with tag filtering, there is a performance hit for filtering on metadata.
    pub filters: Option<serde_json::Value>,
    /// Collection_id specifies the collection to search within. Results will only consist of chunks which are bookmarks within the specified collection.
    pub collection_id: uuid::Uuid,
    #[param(inline)]
    /// Search_type can be either "semantic", "fulltext", or "hybrid". "hybrid" will pull in one page (10 chunks) of both semantic and full-text results then re-rank them using BAAI/bge-reranker-large. "semantic" will pull in one page (10 chunks) of the nearest cosine distant vectors. "fulltext" will pull in one page (10 chunks) of full-text results based on SPLADE.
    pub search_type: String,
    /// Set date_bias to true to bias search results towards more recent chunks. This will work best in hybrid search mode.
    pub date_bias: Option<bool>,
}

impl From<SearchCollectionsData> for SearchChunkData {
    fn from(data: SearchCollectionsData) -> Self {
        Self {
            query: data.query,
            page: data.page,
            link: data.link,
            tag_set: data.tag_set,
            time_range: None,
            filters: data.filters,
            cross_encoder: None,
            weights: None,
            search_type: data.search_type,
            date_bias: data.date_bias,
        }
    }
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct SearchCollectionsResult {
    pub bookmarks: Vec<ScoreChunkDTO>,
    pub collection: ChunkCollection,
    pub total_pages: i64,
}

/// collection_search
///
/// This route allows you to search only within a collection. This is useful for when you only want search results to contain chunks which are members of a specific group. Think about this like searching within a playlist or bookmark folder.
#[utoipa::path(
    post,
    path = "/chunk_collection/search",
    context_path = "/api",
    tag = "chunk_collection",
    request_body(content = SearchCollectionsData, description = "JSON request payload to semantically search a collection", content_type = "application/json"),
    responses(
        (status = 200, description = "Collection chunks which are similar to the embedding vector of the search query", body = SearchCollectionsResult),
        (status = 400, description = "Service error relating to getting the collections that the chunk is in", body = DefaultError),
    ),
)]
#[allow(clippy::too_many_arguments)]
pub async fn search_collections(
    data: web::Json<SearchCollectionsData>,
    pool: web::Data<Pool>,
    _required_user: LoggedUser,
    dataset_org_plan_sub: DatasetAndOrgWithSubAndPlan,
) -> Result<HttpResponse, actix_web::Error> {
    //search over the links as well
    let page = data.page.unwrap_or(1);
    let collection_id = data.collection_id;
    let dataset_id = dataset_org_plan_sub.dataset.id;
    let full_text_search_pool: web::Data<
        r2d2::Pool<diesel::r2d2::ConnectionManager<diesel::prelude::PgConnection>>,
    > = pool.clone();

    let collection = {
        web::block(move || get_collection_by_id_query(collection_id, dataset_id, pool))
            .await
            .map_err(|err| ServiceError::BadRequest(err.to_string()))?
            .map_err(|err| ServiceError::BadRequest(err.message.into()))?
    };

    let parsed_query = parse_query(data.query.clone());

    let result_chunks = match data.search_type.as_str() {
        "fulltext" => {
            search_full_text_collections(
                data,
                parsed_query,
                collection,
                page,
                full_text_search_pool,
                dataset_id,
            )
            .await?
        }
        _ => {
            search_semantic_collections(
                data,
                parsed_query,
                collection,
                page,
                full_text_search_pool,
                dataset_org_plan_sub.dataset,
            )
            .await?
        }
    };

    Ok(HttpResponse::Ok().json(result_chunks))
}

/// get_chunk
///
/// Get a singular chunk by id.
#[utoipa::path(
    get,
    path = "/chunk/{chunk_id}",
    context_path = "/api",
    tag = "chunk",
    responses(
        (status = 200, description = "chunk with the id that you were searching for", body = ChunkMetadata),
        (status = 400, description = "Service error relating to fidning a chunk by tracking_id", body = DefaultError),
    ),
    params(
        ("chunk_id" = Option<uuid>, Path, description = "Id of the chunk you want to fetch.")
    ),
)]
pub async fn get_chunk_by_id(
    chunk_id: web::Path<uuid::Uuid>,
    _user: LoggedUser,
    pool: web::Data<Pool>,
    dataset_org_plan_sub: DatasetAndOrgWithSubAndPlan,
) -> Result<HttpResponse, actix_web::Error> {
    let chunk = web::block(move || {
        get_metadata_from_id_query(chunk_id.into_inner(), dataset_org_plan_sub.dataset.id, pool)
    })
    .await?
    .map_err(|err| ServiceError::BadRequest(err.message.into()))?;

    Ok(HttpResponse::Ok().json(chunk))
}

/// get_chunk_by_tracking_id
///
/// Get a singular chunk by tracking_id. This is useful for when you are coordinating with an external system and want to use your own id as the primary reference for a chunk.
#[utoipa::path(
    get,
    path = "/chunk/tracking_id/{tracking_id}",
    context_path = "/api",
    tag = "chunk",
    responses(
        (status = 200, description = "chunk with the tracking_id that you were searching for", body = ChunkMetadata),
        (status = 400, description = "Service error relating to fidning a chunk by tracking_id", body = DefaultError),
    ),
    params(
        ("tracking_id" = Option<String>, Path, description = "tracking_id of the chunk you want to fetch")
    ),
)]
pub async fn get_chunk_by_tracking_id(
    tracking_id: web::Path<String>,
    _user: LoggedUser,
    pool: web::Data<Pool>,
    _required_user: LoggedUser,
    dataset_org_plan_sub: DatasetAndOrgWithSubAndPlan,
) -> Result<HttpResponse, actix_web::Error> {
    let chunk = web::block(move || {
        get_metadata_from_tracking_id_query(
            tracking_id.into_inner(),
            dataset_org_plan_sub.dataset.id,
            pool,
        )
    })
    .await?
    .map_err(|err| ServiceError::BadRequest(err.message.into()))?;

    Ok(HttpResponse::Ok().json(chunk))
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct RecommendChunksRequest {
    /// The ids of the chunks to be used as positive examples for the recommendation. The chunks in this array will be used to find similar chunks.
    pub positive_chunk_ids: Vec<uuid::Uuid>,
}

/// get_recommended_chunks
///
/// Get recommendations of chunks similar to the chunks in the request. Think about this as a feature similar to the "add to playlist" recommendation feature on Spotify. This request pairs especially well with our collections endpoint.
#[utoipa::path(
    post,
    path = "/chunk/recommend",
    context_path = "/api",
    tag = "chunk",
    request_body(content = RecommendChunksRequest, description = "JSON request payload to get recommendations of chunks similar to the chunks in the request", content_type = "application/json"),
    responses(
        (status = 200, description = "JSON response payload containing chunks with scores which are similar to those in the request body", body = Vec<ChunkMetadataWithFileData>),
        (status = 400, description = "Service error relating to to getting similar chunks", body = DefaultError),
    )
)]
pub async fn get_recommended_chunks(
    data: web::Json<RecommendChunksRequest>,
    pool: web::Data<Pool>,
    _user: LoggedUser,
    dataset_org_plan_sub: DatasetAndOrgWithSubAndPlan,
) -> Result<HttpResponse, actix_web::Error> {
    let positive_chunk_ids = data.positive_chunk_ids.clone();
    let embed_size =
        ServerDatasetConfiguration::from_json(dataset_org_plan_sub.dataset.server_configuration)
            .EMBEDDING_SIZE
            .unwrap_or(1536);

    let recommended_qdrant_point_ids = recommend_qdrant_query(
        positive_chunk_ids,
        dataset_org_plan_sub.dataset.id,
        embed_size,
    )
    .await
    .map_err(|err| {
        ServiceError::BadRequest(format!("Could not get recommended chunks: {}", err))
    })?;

    let recommended_chunk_metadatas =
        web::block(move || get_metadata_from_point_ids(recommended_qdrant_point_ids, pool))
            .await?
            .map_err(|err| {
                ServiceError::BadRequest(format!(
                    "Could not get recommended chunk_metadas from qdrant_point_ids: {}",
                    err
                ))
            })?;

    Ok(HttpResponse::Ok().json(recommended_chunk_metadatas))
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct GenerateChunksRequest {
    /// The model to use for the chat. This can be any model from the model list. If no model is provided, the gryphe/mythomax-l2-13b will be used.
    pub model: Option<String>,
    /// The previous messages to be placed into the chat history. The last message in this array will be the prompt for the model to inference on.
    pub prev_messages: Vec<ChatMessageProxy>,
    /// The ids of the chunks to be retrieved and injected into the context window for RAG.
    pub chunk_ids: Vec<uuid::Uuid>,
}

/// generate_off_chunks
///
/// This endpoint exists as an alternative to the topic+message concept where our API handles chat memory. With this endpoint, the user is responsible for providing the context window and the prompt. See more in the "search before generate" page at docs.trieve.ai.
#[utoipa::path(
    post,
    path = "/chunk/generate",
    context_path = "/api",
    tag = "chunk",
    request_body(content = GenerateChunksRequest, description = "JSON request payload to perform RAG on some chunks (chunks)", content_type = "application/json"),
    responses(
        (status = 200, description = "This will be a HTTP stream of a string, check the chat or search UI for an example how to process this",),
        (status = 400, description = "Service error relating to to updating chunk, likely due to conflicting tracking_id", body = DefaultError),
    ),
)]
pub async fn generate_off_chunks(
    data: web::Json<GenerateChunksRequest>,
    pool: web::Data<Pool>,
    _user: LoggedUser,
    dataset_org_plan_sub: DatasetAndOrgWithSubAndPlan,
) -> Result<HttpResponse, actix_web::Error> {
    let prev_messages = data.prev_messages.clone();
    let chunk_ids = data.chunk_ids.clone();
    let mut chunks = web::block(move || {
        get_metadata_from_ids_query(chunk_ids, dataset_org_plan_sub.dataset.id, pool)
    })
    .await?
    .map_err(|err| ServiceError::BadRequest(err.message.into()))?;

    let openai_api_key = get_env!("OPENROUTER_API_KEY", "OPENROUTER_API_KEY should be set").into();
    let dataset_config =
        ServerDatasetConfiguration::from_json(dataset_org_plan_sub.dataset.server_configuration);
    let base_url = dataset_config
        .LLM_BASE_URL
        .unwrap_or("https://openrouter.ai/v1".into());

    let client = Client {
        api_key: openai_api_key,
        http_client: reqwest::Client::new(),
        base_url,
    };

    let mut messages: Vec<ChatMessage> = prev_messages
        .iter()
        .map(|message| ChatMessage::from(message.clone()))
        .collect();
    messages.truncate(prev_messages.len() - 1);
    messages.push(ChatMessage {
        role: Role::User,
        content: ChatMessageContent::Text("I am going to provide several pieces of information for you to use in response to a request or question. You will not respond until I ask you to.".to_string()),
        tool_calls: None,
        name: None,
        tool_call_id: None,
    });
    messages.push(ChatMessage {
        role: Role::Assistant,
        content: ChatMessageContent::Text(
            "Understood, I will not reply until I receive a direct request or question."
                .to_string(),
        ),
        tool_calls: None,
        name: None,
        tool_call_id: None,
    });
    chunks.sort_by(|a, b| {
        data.chunk_ids
            .iter()
            .position(|&id| id == a.id)
            .unwrap()
            .cmp(&data.chunk_ids.iter().position(|&id| id == b.id).unwrap())
    });
    chunks.iter().enumerate().for_each(|(idx, bookmark)| {
        let first_240_words = bookmark
            .content
            .split_whitespace()
            .take(240)
            .collect::<Vec<_>>()
            .join(" ");

        messages.push(ChatMessage {
            role: Role::User,
            content: ChatMessageContent::Text(format!("Doc {}: {}", idx + 1, first_240_words)),
            tool_calls: None,
            name: None,
            tool_call_id: None,
        });
        messages.push(ChatMessage {
            role: Role::Assistant,
            content: ChatMessageContent::Text("".to_string()),
            tool_calls: None,
            name: None,
            tool_call_id: None,
        });
    });
    messages.push(ChatMessage {
        role: Role::User,
        content: ChatMessageContent::Text(format!("Respond to this question and include the doc numbers that you used in square brackets at the end of the sentences that you used the docs for.: {}",prev_messages
            .last()
            .expect("There needs to be at least 1 prior message")
            .content
            .clone())),
            tool_calls: None,
            name: None,
            tool_call_id: None,
    });

    let parameters = ChatCompletionParameters {
        model: data
            .model
            .clone()
            .unwrap_or("gryphe/mythomax-l2-13b".to_string()),
        messages,
        temperature: None,
        top_p: None,
        n: None,
        stop: None,
        max_tokens: None,
        presence_penalty: Some(0.8),
        frequency_penalty: Some(0.8),
        logit_bias: None,
        user: None,
        response_format: None,
        tools: None,
        tool_choice: None,
        logprobs: None,
        top_logprobs: None,
        seed: None,
    };

    let stream = client.chat().create_stream(parameters).await.unwrap();

    Ok(HttpResponse::Ok().streaming(stream.map(
        move |response| -> Result<Bytes, actix_web::Error> {
            if let Ok(response) = response {
                let chat_content = response.choices[0].delta.content.clone();
                return Ok(Bytes::from(chat_content.unwrap_or("".to_string())));
            }
            Err(ServiceError::InternalServerError(
                "Model Response Error. Please try again later".into(),
            )
            .into())
        },
    )))
}
