// src/player/item_fetcher.rs
use crate::jellyfin::api::{JellyfinApiContract, JellyfinError};
use crate::jellyfin::models::MediaItem;
use crate::player::PLAYER_LOG_TARGET; // Use the log target from the parent
use std::sync::Arc;
use tracing::{error, info, warn, instrument};
use std::collections::HashMap; // Import HashMap

/// Fetches item details from Jellyfin API and sorts them according to the input ID order.
/// Errors are returned to the caller to handle (e.g., broadcasting an update).
#[instrument(skip(jellyfin_client, item_ids), fields(id_count = item_ids.len()))]
pub async fn fetch_and_sort_items(
    jellyfin_client: Arc<dyn JellyfinApiContract>,
    item_ids: &[String],
) -> Result<Vec<MediaItem>, JellyfinError> {
    if item_ids.is_empty() {
        return Ok(Vec::new());
    }
    info!(target: PLAYER_LOG_TARGET, "Fetching details for {} items...", item_ids.len());
    match jellyfin_client.get_items_details(item_ids).await {
        Ok(mut media_items) => {
            info!(target: PLAYER_LOG_TARGET, "Successfully fetched details for {} items.", media_items.len());
            if media_items.len() != item_ids.len() {
                 warn!(target: PLAYER_LOG_TARGET, "Requested {} items but received details for {}. Some items might be invalid or inaccessible.", item_ids.len(), media_items.len());
            }

            // Ensure order matches request
            let original_order: HashMap<&String, usize> = item_ids
                .iter()
                .enumerate()
                .map(|(i, id)| (id, i))
                .collect();

            media_items.sort_by_key(|item| *original_order.get(&item.id).unwrap_or(&usize::MAX));

            // Filter out items that weren't found in the original request (shouldn't happen with sort_by_key, but belt-and-suspenders)
            let final_items = media_items.into_iter().filter(|item| original_order.contains_key(&item.id)).collect::<Vec<_>>();

            if final_items.len() < item_ids.len() {
                 warn!(target: PLAYER_LOG_TARGET, "Final item count ({}) after sorting/filtering is less than requested ({}).", final_items.len(), item_ids.len());
            }

            Ok(final_items)
        }
        Err(e) => {
            error!(target: PLAYER_LOG_TARGET, "Failed to fetch item details: {}", e);
            // Propagate the error for the caller to handle
            Err(e)
        }
    }
}