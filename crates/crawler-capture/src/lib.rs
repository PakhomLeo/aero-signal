//! Aero Signal 文章抓取：identity、正文抽取、图片分类、对象存储、浏览器桩。

pub mod asset_downloader;
pub mod browser;
pub mod capture;
pub mod classifier;
pub mod egress_guard;
pub mod error;
pub mod extractor;
pub mod identity;
pub mod object_store;
pub mod types;

pub use asset_downloader::{
    AssetDownloadOutcome, AssetDownloader, AssetFetchRequest, ByteFetcher, FetchedAsset,
    MemoryByteFetcher, ReqwestByteFetcher,
};
pub use browser::{
    BrowserRenderRequest, BrowserRenderResult, BrowserRuntime, CdpBrowser, DisabledBrowser,
    DomImage, FakeBrowser, merge_lazy_candidates,
};
pub use capture::{
    ArticleFetchClient, FetchedPage, MemoryPageFetcher, PageFetchRequest, PageFetcher,
    ReqwestPageFetcher,
};
pub use classifier::{ImageClassifier, MeasuredImage};
pub use error::{CrawlerError, CrawlerResult};
pub use extractor::{BodyExtractor, ExtractedBody};
pub use identity::{
    ArticleIdentity, ArticleIdentityKind, ArticleIdentityResolver, IdentityInput, canonicalize_url,
};
pub use object_store::{FakeObjectStore, MinioObjectStore, ObjectStore};
pub use types::*;
