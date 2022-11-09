pub mod iot;
pub mod mobile;

pub type GrpcResult<T> = std::result::Result<tonic::Response<T>, tonic::Status>;
pub type VerifyResult<T> = std::result::Result<T, tonic::Status>;
