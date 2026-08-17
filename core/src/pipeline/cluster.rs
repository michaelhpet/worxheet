use hdbscan_rs::{Hdbscan, StoreCenters};
use ndarray::Array2;
use sqlx::SqlitePool;

use super::embed::{bytes_to_embedding, embedding_to_bytes};

/// Cap on the number of context chunks fed into a single generation prompt.
/// At 512 tokens per chunk, 10 chunks plus chat-template overhead stays under
/// the generation model's prompt budget (6144 tokens); 12 would overflow it.
pub const MAX_CONTEXT_CHUNKS: usize = 10;

/// HDBSCAN `min_cluster_size` scales with the number of chunks. Tuned against
/// a real 2552-chunk worksheet: `n/200` (≈13 there) yields ~33 topical clusters.
fn min_cluster_size(n_chunks: usize) -> usize {
    ((n_chunks as f64 / 200.0).round() as usize).clamp(3, 16)
}

/// `min_samples` for HDBSCAN. A small fixed value keeps density estimation
/// smooth in high-dimensional embedding space; larger values collapse the data
/// into one dominant cluster plus noise.
const MIN_SAMPLES: usize = 3;

/// Result of clustering a worksheet's chunk embeddings.
pub struct ClusterAssignment {
    /// Centroid per cluster label (labels are `0..centroids.len()`).
    pub centroids: Vec<Vec<f32>>,
    /// Per-chunk cluster label; `-1` marks HDBSCAN noise.
    pub labels: Vec<i32>,
}

/// Run HDBSCAN over the chunk vectors. Falls back to a single cluster when the
/// worksheet is too small or no stable clusters emerge, so downstream
/// even-sampling always has something to work with. `allow_single_cluster` is
/// left off so the persistent child clusters of a large worksheet are selected
/// instead of one region swallowing the whole document plus noise.
pub fn assign_clusters(vectors: &[Vec<f32>]) -> Result<ClusterAssignment, String> {
    if vectors.is_empty() {
        return Ok(ClusterAssignment {
            centroids: Vec::new(),
            labels: Vec::new(),
        });
    }

    let dimension = vectors[0].len();
    let n_chunks = vectors.len();

    if n_chunks <= 2 {
        return Ok(single_cluster(vectors));
    }

    let mut data = Array2::<f64>::zeros((n_chunks, dimension));
    for (mut row, vector) in data.rows_mut().into_iter().zip(vectors) {
        for (cell, value) in row.iter_mut().zip(vector) {
            *cell = *value as f64;
        }
    }

    let min_size = min_cluster_size(n_chunks).min(n_chunks);
    let params = Hdbscan::builder()
        .min_cluster_size(min_size)
        .min_samples(MIN_SAMPLES.min(n_chunks))
        .allow_single_cluster(false)
        .store_centers(StoreCenters::Centroid)
        .build()
        .map_err(|e| e.to_string())?;

    let mut model = Hdbscan::new(params);
    model
        .fit(&data.view())
        .map_err(|e| format!("Failed to cluster chunks: {e}"))?;

    let labels = model.labels().unwrap_or(&[]).to_vec();
    let n_clusters = labels.iter().copied().max().unwrap_or(-1) + 1;

    if n_clusters <= 0 {
        return Ok(single_cluster(vectors));
    }

    let mut centroids = if let Some(centers) = model.centroids() {
        let mut out = Vec::with_capacity(n_clusters as usize);
        for row in centers.rows() {
            let mut vector: Vec<f32> = row.iter().map(|v| *v as f32).collect();
            normalize(&mut vector);
            out.push(vector);
        }
        while out.len() < n_clusters as usize {
            out.push(vec![0.0; dimension]);
        }
        out
    } else {
        compute_centroids(vectors, &labels)
    };

    centroids.truncate(n_clusters as usize);

    Ok(ClusterAssignment { centroids, labels })
}

fn single_cluster(vectors: &[Vec<f32>]) -> ClusterAssignment {
    let mut centroid = vec![0.0f32; vectors[0].len()];
    for vector in vectors {
        for (sum, value) in centroid.iter_mut().zip(vector) {
            *sum += value;
        }
    }
    let count = vectors.len() as f32;
    for value in &mut centroid {
        *value /= count;
    }
    normalize(&mut centroid);
    ClusterAssignment {
        centroids: vec![centroid],
        labels: vec![0; vectors.len()],
    }
}

fn compute_centroids(vectors: &[Vec<f32>], labels: &[i32]) -> Vec<Vec<f32>> {
    let n_clusters = labels.iter().copied().max().unwrap_or(-1) + 1;
    let dimension = vectors[0].len();
    let mut sums = vec![vec![0.0f32; dimension]; n_clusters as usize];
    let mut counts = vec![0usize; n_clusters as usize];
    for (label, vector) in labels.iter().zip(vectors) {
        if *label < 0 {
            continue;
        }
        let bucket = &mut sums[*label as usize];
        for (sum, value) in bucket.iter_mut().zip(vector) {
            *sum += value;
        }
        counts[*label as usize] += 1;
    }
    let mut centroids = Vec::with_capacity(n_clusters as usize);
    for (bucket, count) in sums.iter().zip(counts) {
        let mut vector = bucket.clone();
        if count > 0 {
            let inv = count as f32;
            for value in &mut vector {
                *value /= inv;
            }
        }
        normalize(&mut vector);
        centroids.push(vector);
    }
    centroids
}

fn normalize(vector: &mut [f32]) {
    let norm: f32 = vector.iter().map(|v| v * v).sum();
    if norm > 0.0 {
        let inv = norm.sqrt().recip();
        for value in vector.iter_mut() {
            *value *= inv;
        }
    }
}

/// Split the material into per-cluster generation units.
///
/// HDBSCAN noise chunks are skipped; only real topic clusters are used. Units
/// are ordered by the source position of each cluster's earliest chunk so
/// concatenated summaries and mind maps read in document order. Each unit holds
/// up to `budget` member indices spread evenly by position from that cluster.
pub fn cluster_contexts(
    cluster_labels: &[i32],
    positions: &[i32],
    budget: usize,
) -> Vec<Vec<usize>> {
    if budget == 0 || cluster_labels.is_empty() {
        return Vec::new();
    }

    let max_label = cluster_labels.iter().copied().max().unwrap_or(-1);
    let mut clusters: Vec<Vec<usize>> = Vec::new();

    for label in 0..=max_label {
        let mut members: Vec<usize> = (0..cluster_labels.len())
            .filter(|index| cluster_labels[*index] == label)
            .collect();
        if members.is_empty() {
            continue;
        }
        members.sort_by(|a, b| positions[*a].cmp(&positions[*b]));
        clusters.push(members);
    }

    clusters.sort_by(|a, b| positions[a[0]].cmp(&positions[b[0]]));

    clusters
        .into_iter()
        .map(|members| pick_evenly(&members, budget))
        .collect()
}

/// Take `count` evenly spaced entries from a position-sorted chunk list.
pub(crate) fn pick_evenly(members: &[usize], count: usize) -> Vec<usize> {
    let n = members.len();
    if count >= n {
        return members.to_vec();
    }
    let stride = n as f64 / count as f64;
    (0..count)
        .map(|i| members[(i as f64 * stride).floor() as usize])
        .collect()
}

/// Recompute HDBSCAN clusters over every embedded chunk of a worksheet and
/// persist the centroids plus each chunk's cluster assignment.
pub async fn rebuild_clusters(pool: &SqlitePool, worksheet_id: &str) -> Result<(), String> {
    let rows = sqlx::query_as::<_, (String, Vec<u8>)>(
        "SELECT id, embedding
         FROM chunks
         WHERE worksheet_id = ? AND embedding IS NOT NULL
         ORDER BY position",
    )
    .bind(worksheet_id)
    .fetch_all(pool)
    .await
    .map_err(|_| String::from("Failed to query embedded chunks"))?;

    if rows.is_empty() {
        return Ok(());
    }

    let ids: Vec<String> = rows.iter().map(|(id, _)| id.clone()).collect();
    let vectors: Vec<Vec<f32>> = rows
        .iter()
        .map(|(_, blob)| bytes_to_embedding(blob))
        .collect::<Result<_, _>>()?;

    let assignment =
        tauri::async_runtime::spawn_blocking(move || assign_clusters(&vectors))
            .await
            .map_err(|e| format!("Clustering task failed: {e}"))??;

    let mut sizes = vec![0usize; assignment.centroids.len()];
    for label in &assignment.labels {
        if *label >= 0 {
            sizes[*label as usize] += 1;
        }
    }

    sqlx::query("DELETE FROM clusters WHERE worksheet_id = ?")
        .bind(worksheet_id)
        .execute(pool)
        .await
        .map_err(|_| String::from("Failed to clear old clusters"))?;

    for (cluster_index, centroid) in assignment.centroids.iter().enumerate() {
        sqlx::query(
            "INSERT INTO clusters (worksheet_id, cluster_index, centroid, size)
             VALUES (?, ?, ?, ?)",
        )
        .bind(worksheet_id)
        .bind(cluster_index as i32)
        .bind(embedding_to_bytes(centroid))
        .bind(sizes[cluster_index] as i64)
        .execute(pool)
        .await
        .map_err(|_| String::from("Failed to store cluster"))?;
    }

    for (id, label) in ids.iter().zip(&assignment.labels) {
        let index: Option<i32> = (*label >= 0).then_some(*label);
        sqlx::query("UPDATE chunks SET cluster_index = ? WHERE id = ?")
            .bind(index)
            .bind(id)
            .execute(pool)
            .await
            .map_err(|_| String::from("Failed to update chunk cluster"))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gaussian(centroid: &[f32], spread: f32, count: usize, seed: u32) -> Vec<Vec<f32>> {
        let mut points = Vec::with_capacity(count);
        for point_index in 0..count {
            let mut point = Vec::with_capacity(centroid.len());
            for (dimension, value) in centroid.iter().enumerate() {
                let noise = ((point_index as f32 * (dimension as f32 + 13.0) + seed as f32) * 7.17)
                    % 1.0
                    - 0.5;
                point.push(value + noise * spread);
            }
            normalize(&mut point);
            points.push(point);
        }
        points
    }

    #[test]
    fn test_assign_clusters_separates_two_groups() {
        let mut vectors = gaussian(&[1.0, 0.5], 0.15, 50, 1);
        vectors.extend(gaussian(&[-1.0, -0.5], 0.15, 50, 2));
        let assignment = assign_clusters(&vectors).expect("should cluster");
        assert!(
            assignment.centroids.len() >= 2,
            "expected at least 2 clusters, got {}",
            assignment.centroids.len()
        );
        assert_eq!(assignment.labels.len(), vectors.len());
    }

    #[test]
    fn test_assign_clusters_tiny_input_single_cluster() {
        let vectors = gaussian(&[1.0, 0.5], 0.1, 2, 3);
        let assignment = assign_clusters(&vectors).expect("should fall back");
        assert_eq!(assignment.centroids.len(), 1);
        assert_eq!(assignment.labels, vec![0, 0]);
    }

    #[test]
    fn test_cluster_contexts_skips_noise_and_orders_by_position() {
        let labels = vec![-1, 1, 1, 0, 0, -1, 1];
        let positions: Vec<i32> = vec![0, 1, 2, 3, 4, 5, 6];
        let units = cluster_contexts(&labels, &positions, 10);
        assert_eq!(units.len(), 2, "noise chunks must be skipped");
        assert_eq!(
            units[0],
            vec![1, 2, 6],
            "cluster 1 starts earliest at position 1"
        );
        assert_eq!(units[1], vec![3, 4], "cluster 0 starts at position 3");
    }

    #[test]
    fn test_cluster_contexts_respects_budget_per_unit() {
        let labels = vec![0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        let positions: Vec<i32> = (0..12).collect();
        let units = cluster_contexts(&labels, &positions, 10);
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].len(), 10, "budget caps each unit");
        assert_eq!(units[0][0], 0);
    }
}