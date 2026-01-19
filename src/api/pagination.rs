use futures::{Stream, StreamExt};

/// Paginate a stream of items
/// Returns (items, count) where count is the number of items returned
pub async fn paginate<T, S>(stream: S, page: usize, page_size: usize) -> (Vec<T>, usize)
where
    S: Stream<Item = T>,
{
    let page = page.max(1);
    let page_size = page_size.min(100).max(1);
    let skip = (page - 1) * page_size;

    let items: Vec<T> = stream.skip(skip).take(page_size).collect().await;

    let count = items.len();
    (items, count)
}
