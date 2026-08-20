// GoBGP v4 api 最小手写 stub（仅 AddPath / DeletePath / ListPath），服务名 api.GoBgpService
pub mod family {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, ::prost::Enumeration)]
    #[repr(i32)]
    pub enum Afi {
        Unspecified = 0,
        Ip = 1,
        Ip6 = 2,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, ::prost::Enumeration)]
    #[repr(i32)]
    pub enum Safi {
        Unspecified = 0,
        Unicast = 1,
    }
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Family {
    #[prost(enumeration = "family::Afi", tag = "1")]
    pub afi: i32,
    #[prost(enumeration = "family::Safi", tag = "2")]
    pub safi: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, ::prost::Enumeration)]
#[repr(i32)]
pub enum TableType {
    Unspecified = 0,
    Global = 1,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct IpAddressPrefix {
    #[prost(uint32, tag = "1")]
    pub prefix_len: u32,
    #[prost(string, tag = "2")]
    pub prefix: String,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Nlri {
    #[prost(oneof = "nlri::Payload", tags = "1")]
    pub nlri: Option<nlri::Payload>,
}

pub mod nlri {
    #[derive(Clone, PartialEq, ::prost::Oneof)]
    pub enum Payload {
        #[prost(message, tag = "1")]
        Prefix(super::IpAddressPrefix),
    }
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct OriginAttribute {
    #[prost(uint32, tag = "1")]
    pub origin: u32,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct NextHopAttribute {
    #[prost(string, tag = "1")]
    pub next_hop: String,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct CommunitiesAttribute {
    #[prost(uint32, repeated, packed = "true", tag = "1")]
    pub communities: Vec<u32>,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Attribute {
    #[prost(oneof = "attribute::Attr", tags = "2, 4, 9")]
    pub attr: Option<attribute::Attr>,
}

pub mod attribute {
    #[derive(Clone, PartialEq, ::prost::Oneof)]
    pub enum Attr {
        #[prost(message, tag = "2")]
        Origin(super::OriginAttribute),
        #[prost(message, tag = "4")]
        NextHop(super::NextHopAttribute),
        #[prost(message, tag = "9")]
        Communities(super::CommunitiesAttribute),
    }
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Path {
    #[prost(message, optional, tag = "1")]
    pub nlri: Option<Nlri>,
    #[prost(message, repeated, tag = "2")]
    pub pattrs: Vec<Attribute>,
    #[prost(bool, tag = "5")]
    pub is_withdraw: bool,
    #[prost(bool, tag = "8")]
    pub no_implicit_withdraw: bool,
    #[prost(message, optional, tag = "9")]
    pub family: Option<Family>,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct AddPathRequest {
    #[prost(enumeration = "TableType", tag = "1")]
    pub table_type: i32,
    #[prost(string, tag = "2")]
    pub vrf_id: String,
    #[prost(message, optional, tag = "3")]
    pub path: Option<Path>,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct AddPathResponse {
    #[prost(bytes = "vec", tag = "1")]
    pub uuid: Vec<u8>,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct DeletePathRequest {
    #[prost(enumeration = "TableType", tag = "1")]
    pub table_type: i32,
    #[prost(string, tag = "2")]
    pub vrf_id: String,
    #[prost(message, optional, tag = "3")]
    pub family: Option<Family>,
    #[prost(message, optional, tag = "4")]
    pub path: Option<Path>,
    #[prost(bytes = "vec", tag = "5")]
    pub uuid: Vec<u8>,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct DeletePathResponse {}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct TableLookupPrefix {
    #[prost(string, tag = "1")]
    pub prefix: String,
    #[prost(enumeration = "table_lookup_prefix::Type", tag = "2")]
    pub r#type: i32,
}

pub mod table_lookup_prefix {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, ::prost::Enumeration)]
    #[repr(i32)]
    pub enum Type {
        Unspecified = 0,
        Exact = 1,
        Longer = 2,
        Shorter = 3,
    }
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct ListPathRequest {
    #[prost(enumeration = "TableType", tag = "1")]
    pub table_type: i32,
    #[prost(string, tag = "2")]
    pub name: String,
    #[prost(message, optional, tag = "3")]
    pub family: Option<Family>,
    #[prost(message, repeated, tag = "4")]
    pub prefixes: Vec<TableLookupPrefix>,
    #[prost(enumeration = "list_path_request::SortType", tag = "5")]
    pub sort_type: i32,
    #[prost(bool, tag = "6")]
    pub enable_filtered: bool,
    #[prost(bool, tag = "7")]
    pub enable_nlri_binary: bool,
    #[prost(bool, tag = "8")]
    pub enable_attribute_binary: bool,
    #[prost(bool, tag = "9")]
    pub enable_only_binary: bool,
}

pub mod list_path_request {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, ::prost::Enumeration)]
    #[repr(i32)]
    pub enum SortType {
        Unspecified = 0,
        Prefix = 1,
    }
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Destination {
    #[prost(string, tag = "1")]
    pub prefix: String,
    #[prost(message, repeated, tag = "2")]
    pub paths: Vec<Path>,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct ListPathResponse {
    #[prost(message, optional, tag = "1")]
    pub destination: Option<Destination>,
}

pub mod gobgp_api_client {
    use tonic::codegen::{http, Body, Bytes, StdError};

    #[derive(Debug, Clone)]
    pub struct GobgpApiClient<T> {
        inner: tonic::client::Grpc<T>,
    }

    impl GobgpApiClient<tonic::transport::Channel> {
        pub async fn connect<D>(dst: D) -> Result<Self, tonic::transport::Error>
        where
            D: TryInto<tonic::transport::Endpoint>,
            D::Error: Into<StdError>,
        {
            let conn = tonic::transport::Endpoint::new(dst)?.connect().await?;
            Ok(Self::new(conn))
        }
    }

    impl<T> GobgpApiClient<T>
    where
        T: tonic::client::GrpcService<tonic::body::Body>,
        T::Error: Into<StdError>,
        T::ResponseBody: Body<Data = Bytes> + Send + 'static,
        <T::ResponseBody as Body>::Error: Into<StdError> + Send,
    {
        pub fn new(inner: T) -> Self {
            let inner = tonic::client::Grpc::new(inner);
            Self { inner }
        }

        async fn ready(&mut self) -> Result<(), tonic::Status> {
            self.inner
                .ready()
                .await
                .map_err(|e| tonic::Status::unknown(format!("gobgp api not ready: {}", e.into())))
        }

        pub async fn add_path(
            &mut self,
            request: impl tonic::IntoRequest<super::AddPathRequest>,
        ) -> Result<tonic::Response<super::AddPathResponse>, tonic::Status> {
            self.ready().await?;
            let path = http::uri::PathAndQuery::from_static("/api.GoBgpService/AddPath");
            let codec = tonic_prost::ProstCodec::default();
            self.inner.unary(request.into_request(), path, codec).await
        }

        pub async fn delete_path(
            &mut self,
            request: impl tonic::IntoRequest<super::DeletePathRequest>,
        ) -> Result<tonic::Response<super::DeletePathResponse>, tonic::Status> {
            self.ready().await?;
            let path = http::uri::PathAndQuery::from_static("/api.GoBgpService/DeletePath");
            let codec = tonic_prost::ProstCodec::default();
            self.inner.unary(request.into_request(), path, codec).await
        }

        pub async fn list_path(
            &mut self,
            request: impl tonic::IntoRequest<super::ListPathRequest>,
        ) -> Result<tonic::Response<tonic::codec::Streaming<super::ListPathResponse>>, tonic::Status>
        {
            self.ready().await?;
            let path = http::uri::PathAndQuery::from_static("/api.GoBgpService/ListPath");
            let codec = tonic_prost::ProstCodec::default();
            self.inner
                .server_streaming(request.into_request(), path, codec)
                .await
        }
    }
}
