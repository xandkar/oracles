use crate::{BytesMutStream, Error};
use async_compression::tokio::bufread::GzipDecoder;
use futures::{
    stream::{self},
    StreamExt, TryFutureExt, TryStreamExt,
};
use std::path::{Path, PathBuf};
use tokio::{fs::File, io::BufReader};
use tokio_util::codec::{length_delimited::LengthDelimitedCodec, FramedRead};

pub fn source<I, P>(paths: I) -> BytesMutStream
where
    I: IntoIterator<Item = P>,
    P: AsRef<Path>,
{
    let paths: Vec<PathBuf> = paths
        .into_iter()
        .map(|path| path.as_ref().to_path_buf())
        .collect();
    stream::iter(paths)
        .map(|path| File::open(path).map_err(Error::from))
        .buffered(2)
        .flat_map(|file| match file {
            Ok(file) => {
                let buf_reader = BufReader::new(file);
                FramedRead::new(GzipDecoder::new(buf_reader), LengthDelimitedCodec::new())
                    .map_err(Error::from)
                    .boxed()
            }
            Err(err) => stream::once(async { Err(err) }).boxed(),
        })
        .boxed()
    // stream::try_unfold(
    //     (paths, None),
    //     |(mut paths, current): (Vec<PathBuf>, Option<ByteStream>)| async move {
    //         if let Some(mut stream) = current {
    //             match stream.next().await {
    //                 Some(Ok(item)) => return Ok(Some((item, (paths, Some(stream))))),
    //                 Some(Err(err)) => return Err(err),
    //                 None => (),
    //             }
    //         };
    //         // No current exhausted or none. Pop paths and make a new file_source
    //         while let Some(path) = paths.pop() {
    //             let mut stream = File::open(&path)
    //                 .map_ok(stream_source)
    //                 .map_err(Error::from)
    //                 .await?;
    //             match stream.next().await {
    //                 Some(Ok(item)) => return Ok(Some((item, (paths, Some(stream))))),
    //                 Some(Err(err)) => return Err(err),
    //                 None => (),
    //             }
    //         }
    //         Ok(None)
    //     },
    // )
    // .boxed()
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::file_store::FileStore;
    use std::str::FromStr;

    fn infos(names: &[&str]) -> Vec<FileInfo> {
        names
            .iter()
            .map(|v| FileInfo::from_str(v).expect("valid file_info"))
            .collect()
    }

    #[tokio::test]
    #[ignore = "credentials required"]
    async fn test_multi_read() {
        //
        // Run with `cargo test -- --include-ignored`
        //
        // Use FileStore::get. These two files exist in the devnet bucket:
        //
        // aws s3 ls s3://devnet-poc5g-rewards
        // 2022-08-05 15:35:55     240363 cell_heartbeat.1658832527866.gz
        // 2022-08-05 15:36:08    6525274 cell_heartbeat.1658834120042.gz
        //
        let file_store = FileStore::new(None, "us-east-1").await.expect("file store");
        let stream = store_source(
            file_store.clone(),
            "devnet-poc5g-rewards",
            infos(&[
                "cell_heartbeat.1658832527866.gz",
                "cell_heartbeat.1658834120042.gz",
            ]),
        );
        let p1_stream = store_source(
            file_store.clone(),
            "devnet-poc5g-rewards",
            infos(&["cell_heartbeat.1658832527866.gz"]),
        );
        let p2_stream = store_source(
            file_store.clone(),
            "devnet-poc5g-rewards",
            infos(&["cell_heartbeat.1658834120042.gz"]),
        );

        let p1_count = p1_stream.count().await;
        let p2_count = p2_stream.count().await;
        let multi_count = stream.count().await;

        assert_eq!(multi_count, p1_count + p2_count);
    }
}
