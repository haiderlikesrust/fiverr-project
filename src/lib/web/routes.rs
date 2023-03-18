use std::{io, str::FromStr, sync::Arc};

use axum::{
    body::StreamBody,
    extract::{Multipart, Path},
    http::{header::COOKIE, HeaderMap, StatusCode},
    response::IntoResponse,
    Extension, Json, TypedHeader,
};
use headers::ContentType;
use tower_cookies::{Cookie, Cookies};
use uuid::Uuid;

use crate::{
    database::{actions::DatabaseHand, Database},
    error::ApiError,
    models::{self, ImageLink, Listing, Product, ResponseUser, ServerStatus, User},
    web::{ImageData, ReqId},
    State,
};
use tokio::fs::File as AsyncFile;
use tokio::io::BufWriter as AsyncBufWriter;

use axum::body::Bytes;
use axum::BoxError;
use futures::{Stream, TryStreamExt};

use tokio_util::io::{ReaderStream, StreamReader};

use super::{
    BoxCreation, DeleteListing, IdAndReqId, ProductCreation, Register, ReqListing, SignIn,
};

pub async fn register_user(
    Extension(data): Extension<Arc<State>>,
    user: Json<Register>,
    cookies: Cookies,
) -> Result<Json<ResponseUser>, ApiError> {
    let pool = data.database.pool.clone();
    let user_data: User = user.0.clone().try_into()?;
    let response = DatabaseHand::create_user(&pool, &user_data).await?;
    let private_key = DatabaseHand::get_private_key(&pool, &user_data.id)
        .await?
        .to_string();
    cookies.add(Cookie::new("session_id", private_key));
    Ok(Json(response))
}

pub async fn sign_in_user(
    Extension(data): Extension<Arc<State>>,
    user: Json<SignIn>,
    cookies: Cookies,
) -> Result<Json<ResponseUser>, ApiError> {
    let pool = data.database.pool.clone();
    let user_data = user.0.clone();
    let response = DatabaseHand::sign_in(&pool, &user_data).await?;
    let private_key = DatabaseHand::get_private_key(&pool, &response.id)
        .await?
        .to_string();
    cookies.add(Cookie::new("session_id", private_key));
    Ok(Json(response))
}
pub async fn get_all_users(
    Extension(data): Extension<Arc<State>>,
) -> Result<Json<Vec<ResponseUser>>, ApiError> {
    let pool = data.database.pool.clone();
    let users = DatabaseHand::get_users(&pool).await?;
    Ok(Json(users))
}
pub async fn auth(
    Extension(data): Extension<Arc<State>>,
    headers: HeaderMap,
) -> Result<Json<ResponseUser>, ApiError> {
    let pool = data.database.pool.clone();
    match headers.get(COOKIE) {
        Some(data) => {
            let cookie = data.to_str().unwrap().split('=').collect::<Vec<_>>()[1];
            let key: String = cookie.to_string();
            let user =
                DatabaseHand::get_user_from_private_key(&pool, &Uuid::from_str(&key).unwrap())
                    .await?;
            Ok(Json(user))
        }
        None => Err(ApiError::NoSessionCookieFound),
    }
}

pub async fn get_listings(
    Extension(data): Extension<Arc<State>>,
) -> Result<Json<Vec<Listing>>, ApiError> {
    let pool = data.database.pool.clone();
    let listings = DatabaseHand::get_listing(&pool).await?;
    Ok(Json(listings))
}

pub async fn create_listing(
    Extension(data): Extension<Arc<State>>,
    mut form: Multipart,
) -> Result<Json<Vec<Listing>>, ApiError> {
    let pool = data.database.pool.clone();
    let mut req_list = ReqListing {
        tty: String::new(),
        title: String::new(),
        image: String::new(),
        req_id: String::new(),
    };
    let mut file_name = String::from("database/images/");
    while let Some(f) = form.next_field().await.unwrap() {
        if let Some(name) = f.name() {
            match name {
                "title" => {
                    let value = f.text().await?;
                    req_list.title = value;
                }
                "req_id" => {
                    let value = f.text().await?;
                    req_list.req_id = value;
                }
                "tty" => {
                    let value = f.text().await?;
                    req_list.tty = value;
                }
                "file" => match f.content_type() {
                    Some("image/png") => {
                        let id = uuid::Uuid::new_v4().to_string();
                        file_name.push_str(&id);
                        stream_to_file(&format!("{id}.png"), f).await.unwrap();
                    }
                    _ => (),
                },
                _ => (),
            }
        }
    }

    let listing: Listing = req_list.clone().into();
    let req_id: ReqId = req_list.into();
    let image_data = ImageData {
        path: file_name,
        id: listing.clone().id,
    };

    let listings = DatabaseHand::create_listing(&pool, (listing, req_id, image_data)).await?;
    Ok(Json(listings))
}

pub async fn stream_to_file<S, E>(path: &str, stream: S) -> Result<(), (StatusCode, String)>
where
    S: Stream<Item = Result<Bytes, E>>,
    E: Into<BoxError>,
{
    async {
        let body_with_io_error = stream.map_err(|err| io::Error::new(io::ErrorKind::Other, err));
        let body_reader = StreamReader::new(body_with_io_error);
        futures::pin_mut!(body_reader);

        let path = std::path::Path::new("database/images/").join(path);
        let mut file = AsyncBufWriter::new(AsyncFile::create(path).await?);

        tokio::io::copy(&mut body_reader, &mut file).await?;

        Ok::<_, io::Error>(())
    }
    .await
    .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))
}

pub async fn generate_link(
    Extension(data): Extension<Arc<State>>,
    mut form: Multipart,
) -> Result<Json<ImageLink>, ApiError> {
    let id = uuid::Uuid::new_v4();

    let img = ImageData {
        path: String::new(),
        id: id.clone(),
    };

    let mut file_name = String::from("database/images/");
    while let Some(f) = form.next_field().await.unwrap() {
        if let Some(name) = f.name() {
            match name {
                "file" => match f.content_type() {
                    Some("image/png") => {
                        file_name.push_str(&id.to_string());
                        stream_to_file(&format!("{id}.png"), f).await.unwrap();
                    }
                    _ => (),
                },
                _ => (),
            }
        }
    }
    let img_clone = img.clone();
    DatabaseHand::save_image(&data.database.pool.clone(), img).await?;
    Ok(Json(ImageLink {
        link: img_clone.id.to_string(),
    }))
}
pub async fn create_box(
    Extension(data): Extension<Arc<State>>,
    box_data: Json<BoxCreation>,
) -> Result<Json<Vec<models::Box>>, ApiError> {
    let pool = data.database.pool.clone();
    let box_data = box_data.0.into();
    let bx = DatabaseHand::create_box(&pool, box_data).await?;
    Ok(Json(bx))
}

pub async fn delete_listing(
    Extension(data): Extension<Arc<State>>,
    listing_data: Json<DeleteListing>,
) -> Result<Json<Vec<Listing>>, ApiError> {
    let pool = data.database.pool.clone();
    let listings = DatabaseHand::delete_listing(&pool, listing_data.0.clone().into()).await?;
    Ok(Json(listings))
}

pub async fn delete_single_product(
    Extension(data): Extension<Arc<State>>,
    product_data: Json<IdAndReqId>,
) -> Result<Json<Vec<Listing>>, ApiError> {
    let pool = data.database.pool.clone();
    let products = DatabaseHand::delete_product(&pool, product_data.0.clone().into()).await?;
    Ok(Json(products))
}

pub async fn send_server_status() -> Result<Json<ServerStatus>, ApiError> {
    let status = ServerStatus {
        status: true,
        message: "Server is up and running".to_string(),
    };
    Ok(Json(status))
}

pub async fn add_product_to_box(
    Extension(data): Extension<Arc<State>>,
    product_data: Json<ProductCreation>,
) -> Result<Json<Vec<Listing>>, ApiError> {
    let pool = data.database.pool.clone();
    let product_data: (Vec<Product>, ReqId, Uuid) = product_data.0.into();
    let listing =
        DatabaseHand::add_product_to_box(&pool, (product_data.1, product_data.2, product_data.0))
            .await?;
    Ok(Json(listing))
}

pub async fn delete_box(
    Extension(data): Extension<Arc<State>>,
    box_data: Json<IdAndReqId>,
) -> Result<Json<Vec<Listing>>, ApiError> {
    let pool = data.database.pool.clone();
    let _ = DatabaseHand::delete_box(&pool, box_data.0.clone().into()).await?;
    Ok(Json(DatabaseHand::get_listing(&pool).await?))
}
pub async fn get_image(Path(id): Path<String>) -> Result<impl IntoResponse, ApiError> {
    let (file, raw_path) = match tokio::fs::File::open(format!("database/images/{id}.png")).await {
        Ok(file) => (file, format!("database/images/{id}.png")),
        Err(_) => return Err(ApiError::ImageNotFound),
    };
    let stream = ReaderStream::new(file);
    let body = StreamBody::new(stream);
    match raw_path.split('.').collect::<Vec<_>>()[1] {
        "png" => {
            let he = TypedHeader(ContentType::from(mime::IMAGE_PNG));
            return Ok((he, body));
        }
        _ => Err(ApiError::ImageNotFound),
    }
}
