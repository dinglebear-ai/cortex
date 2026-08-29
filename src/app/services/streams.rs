use super::*;

impl CortexService {
    pub async fn durable_stream_page(
        &self,
        params: db::DurableStreamParams,
    ) -> ServiceResult<db::DurableStreamPage> {
        self.run_db("durable_stream_page", move |pool| {
            db::durable_stream_page(pool, &params)
        })
        .await
    }
}
