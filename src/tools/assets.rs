//! Asset tools: finding the asset server, tuning the store, and handing one
//! session resource to a client as a file it can look at or listen to.
//!
//! [`Cyberdemon::get_asset_url`] is the convenience entry point: it takes a
//! name, decides what kind of resource the bytes are, and dispatches to the
//! converters the dedicated tools already use - [`load_texture`],
//! [`decode_flat_bytes`] and [`decode_patch_bytes`] for pictures,
//! [`sound_to_playable`] and [`music_to_playable`] for audio,
//! [`swatch_image`] for `PLAYPAL` and [`colormap_image`] for `COLORMAP`.
//! [`family`] is the whole dispatch table; nothing here guesses: the
//! classifier's [`LumpKind`], the lump's namespace and the payload's leading
//! bytes decide, and anything left over is refused with the raw-bytes way
//! out named.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData, tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::assets::server::AssetRef;
use crate::assets::store::{MAX_ASSET_BYTES, mime_for_extension};
use crate::error::CyberError;
use crate::gfx::codec::{decode_image, encode_png, is_png};
use crate::gfx::grab::{GrabOffsets, read_grab};
use crate::gfx::image::{IndexedImage, RgbaImage};
use crate::gfx::palette::Palette;
use crate::gfx::patch::Patch;
use crate::music::{MusicKind, sniff_music};
use crate::server::Cyberdemon;
use crate::session::Session;
use crate::state::AppState;
use crate::tools::audio::{DEFAULT_BITS, is_sound_kind, sound_to_playable};
use crate::tools::graphics::resolve::{decode_flat_bytes, decode_patch_bytes, load_texture};
use crate::tools::lumps::rows::LumpRow;
use crate::tools::lumps::select::{LumpSelector, resolve};
use crate::tools::music::music_to_playable;
use crate::tools::palette::{load_palette, swatch_image};
use crate::tools::textures::TextureSet;
use crate::tools::{fail, ok};
use crate::wad::classify::{LumpKind, Namespace, is_doom_patch};
use crate::wad::name::LumpName;

/// The `PLAYPAL` entry pictures are coloured with unless `palette` says
/// otherwise: the normal palette.
const DEFAULT_PALETTE: usize = 0_usize;
/// The `PLAYPAL` entry a `COLORMAP` grid is drawn through: the tables map
/// that palette's entries onto their darkened copies.
const COLORMAP_PALETTE: usize = 0_usize;
/// Colour entries one `COLORMAP` light table maps: the image's width.
const COLORMAP_TABLE: usize = 256_usize;
/// Columns of the `COLORMAP` preview, one per palette index.
const COLORMAP_COLUMNS: u32 = 256_u32;
/// Alpha byte of every preview pixel written by hand.
const OPAQUE: u8 = 255_u8;
/// `RGBA` bytes per pixel, the factor that turns a table's entries into the
/// grid's buffer length.
const RGBA_BYTES: usize = 4_usize;
/// An entirely black pixel, for a table entry the palette does not hold.
const BLACK: [u8; 3] = [0_u8; 3];

/// Arguments for [`Cyberdemon::set_asset_capacity`].
#[derive(Debug, Deserialize, JsonSchema)]
struct SetAssetCapacityParams {
    /// New number of most recent assets the store keeps, within
    /// `1..=10000`; shrinking the store below its current fill evicts the
    /// oldest assets immediately.
    capacity: usize,
}

/// Arguments for [`Cyberdemon::get_asset_url`].
#[derive(Debug, Deserialize, JsonSchema)]
struct GetAssetUrlParams {
    /// The lump selector: `name` plus optional `scope`, `namespace` and
    /// `occurrence` (see `skill/reference/lumps.md`).
    #[serde(flatten)]
    selector: LumpSelector,
    /// Which `PLAYPAL` entry colours an indexed picture or a palette swatch
    /// (default `0`). A `COLORMAP` grid always uses entry `0`, the palette
    /// its tables were built against.
    #[serde(default)]
    palette: Option<usize>,
    /// What a name that both a lump and a `TEXTURE1`/`TEXTURE2` definition
    /// owns means: `"lump"` (the default) publishes the selected lump,
    /// `"texture"` composites the definition. A texture name no lump carries
    /// composites either way.
    #[serde(default)]
    prefer: Option<String>,
}

/// Which resource a name shared by a lump and a composite texture means.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Prefer {
    /// The lump the selector picks.
    Lump,
    /// The `TEXTURE1`/`TEXTURE2` definition.
    Texture,
}

impl Prefer {
    /// The client's `prefer` word, defaulting to [`Prefer::Lump`].
    ///
    /// # Errors
    ///
    /// [`CyberError::InvalidArgument`] for any other name.
    fn parse(text: Option<&str>) -> Result<Self, CyberError> {
        match text {
            None | Some("lump") => Ok(Self::Lump),
            Some("texture") => Ok(Self::Texture),
            Some(other) => Err(CyberError::InvalidArgument(format!(
                "prefer {other:?} is unknown: pass \"lump\" for the selected lump or \"texture\" \
                 for the TEXTURE1/TEXTURE2 definition of that name"
            ))),
        }
    }
}

/// Structured result of [`Cyberdemon::get_asset_server_info`].
#[derive(Serialize)]
struct AssetServerInfo {
    /// The origin every asset `URL` of this session shares, e.g.
    /// `http://localhost:41234`.
    base_url: String,
    /// The `URL` to `PUT` a file body to; replace `{filename}` with the
    /// name to store it under.
    upload_url_template: String,
    /// The maximum number of assets the store keeps.
    capacity: usize,
    /// The number of assets currently stored.
    stored: usize,
    /// The total bytes held by the stored assets.
    stored_bytes: usize,
    /// The largest single asset the store accepts, in bytes.
    max_asset_bytes: usize,
}

/// Structured result of [`Cyberdemon::set_asset_capacity`].
#[derive(Serialize)]
struct CapacityReport {
    /// The capacity the store now has.
    capacity: usize,
    /// The number of oldest assets the change evicted.
    evicted: usize,
}

/// Structured result of [`Cyberdemon::get_asset_url`].
#[derive(Serialize)]
struct AssetUrlReport {
    /// The resource's name, normalised to a lump name.
    name: LumpName,
    /// What the name turned out to be: `texture`, `flat`, `patch`, `sprite`,
    /// `image`, `sound`, `music`, `palette` or `colormap`.
    resolved_as: &'static str,
    /// The media type of the published file.
    mime: String,
    /// The published file.
    asset: AssetRef,
    /// Pixels per row, for a picture.
    #[serde(skip_serializing_if = "Option::is_none")]
    width: Option<u32>,
    /// Rows, for a picture.
    #[serde(skip_serializing_if = "Option::is_none")]
    height: Option<u32>,
}

/// One converted resource, waiting for the asset store.
struct Prepared {
    /// The `resolved_as` word the client sees.
    resolved_as: &'static str,
    /// The published file's extension without the dot, which also names its
    /// media type.
    extension: &'static str,
    /// The file's bytes.
    data: Vec<u8>,
    /// The pixel size, which only the picture families have.
    size: Option<(u32, u32)>,
}

/// The validated arguments of one `get_asset_url` call.
#[derive(Debug)]
struct Request<'req> {
    /// The name to resolve, normalised.
    name: LumpName,
    /// What a name both a lump and a texture owns means.
    prefer: Prefer,
    /// The lump half of the arguments, for [`resolve`].
    selector: &'req LumpSelector,
}

impl<'req> Request<'req> {
    /// Parses one call's arguments.
    ///
    /// # Errors
    ///
    /// [`LumpName::parse`] and [`Prefer::parse`]' [`CyberError::InvalidArgument`]
    /// for a name or a `prefer` word that does not parse.
    fn new(args: &'req GetAssetUrlParams) -> Result<Self, CyberError> {
        Ok(Self {
            name: LumpName::parse(&args.selector.name)?,
            prefer: Prefer::parse(args.prefer.as_deref())?,
            selector: &args.selector,
        })
    }
}

/// What a picture converter needs besides the lump: the session to read
/// colours from and the palette entry to colour with.
struct View<'sess> {
    /// The session whose `PLAYPAL` and patches are read.
    session: &'sess Session,
    /// The requested `PLAYPAL` entry.
    palette: usize,
}

impl View<'_> {
    /// The view of one call.
    const fn new(session: &Session, palette: usize) -> View<'_> {
        View { session, palette }
    }

    /// The session's requested palette.
    ///
    /// # Errors
    ///
    /// [`load_palette`]' [`CyberError::NotFound`] for a session without a
    /// `PLAYPAL` or an entry past its last.
    fn palette(&self) -> Result<Palette, CyberError> {
        load_palette(self.session, self.palette)
    }
}

/// The converter family one lump's bytes select.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Family {
    /// A `PLAYPAL` plane flat, or a `PNG` sitting in the flats region.
    Flat,
    /// A `Doom` patch or sprite picture, with drawing offsets.
    Picture,
    /// An embedded `PNG`, `JPEG` or `GIF`.
    Image,
    /// A `DMX`, `PC` speaker or container sound.
    Sound,
    /// `MUS`, `MIDI` or a tracker module.
    Music,
    /// A `PLAYPAL` colour table.
    Palette,
    /// A `COLORMAP` light-table set.
    Colormap,
    /// Nothing to look at or listen to.
    Other,
}

/// The dispatch table: the classified kind, the region the lump sits in and
/// its leading bytes decide which converter owns it, in that order of
/// specificity.
fn family(kind: LumpKind, namespace: Option<Namespace>, data: &[u8]) -> Family {
    if is_flat(kind, namespace) {
        Family::Flat
    } else if is_picture(kind, data) {
        Family::Picture
    } else if matches!(kind, LumpKind::Png | LumpKind::Jpeg | LumpKind::Gif) {
        Family::Image
    } else if is_sound_kind(kind) {
        Family::Sound
    } else if is_music(kind, data) {
        Family::Music
    } else if kind == LumpKind::Palette {
        Family::Palette
    } else if is_colormap(kind, namespace) {
        Family::Colormap
    } else {
        Family::Other
    }
}

/// Whether one lump is a flat: the classifier's verdict, or simply sitting
/// in the flats region.
fn is_flat(kind: LumpKind, namespace: Option<Namespace>) -> bool {
    kind == LumpKind::Flat || namespace == Some(Namespace::Flats)
}

/// Whether one lump is a patch-format picture: the patches or sprites
/// region's verdict, or bytes a `Doom` patch header describes.
fn is_picture(kind: LumpKind, data: &[u8]) -> bool {
    matches!(kind, LumpKind::Patch | LumpKind::Sprite) || is_doom_patch(data)
}

/// Whether one lump is music: a `MUS` or `MIDI` classification, or bytes a
/// music container owns, which is also how a tracker module gets in.
fn is_music(kind: LumpKind, data: &[u8]) -> bool {
    matches!(kind, LumpKind::Mus | LumpKind::Midi) || sniff_music(data) != MusicKind::Unknown
}

/// Whether one lump is a light-table set: the classifier's verdict, or the
/// `colormaps` region.
fn is_colormap(kind: LumpKind, namespace: Option<Namespace>) -> bool {
    kind == LumpKind::Colormap || namespace == Some(Namespace::Colormaps)
}

/// The converted file behind a `get_asset_url` call, resolved and read under
/// the session lock: the texture when `prefer` asks for it and the name has
/// one, otherwise the selected lump, otherwise the texture of that name.
///
/// # Errors
///
/// Whatever the chosen converter reports, plus [`resolve`]' errors for the
/// selector.
fn prepare(
    session: &Session,
    request: &Request<'_>,
    palette: usize,
) -> Result<Prepared, CyberError> {
    let view = View::new(session, palette);
    if request.prefer == Prefer::Texture && holds_texture(session, request.name)? {
        return texture_asset(&view, request.name);
    }
    match resolve(session, request.selector) {
        Ok(row) => convert(&view, &row),
        Err(error) => fallback_to_texture(&view, request, error),
    }
}

/// The composite texture behind a name no lump answered for, the selector's
/// own failure when no definition carries it either, and any failure that
/// was not a missed name.
///
/// # Errors
///
/// `error` unchanged when it was not a missed name, `error` widened to say
/// the definitions were tried too when no definition carries the name, and
/// [`texture_asset`]' errors when the definition itself cannot be drawn.
fn fallback_to_texture(
    view: &View<'_>,
    request: &Request<'_>,
    error: CyberError,
) -> Result<Prepared, CyberError> {
    if error.code() != "not_found" {
        return Err(error);
    }
    if !holds_texture(view.session, request.name)? {
        return Err(CyberError::NotFound(format!(
            "{error}; {} is not a composite texture either; list_textures names the definitions",
            request.name
        )));
    }
    texture_asset(view, request.name)
}

/// Whether the merged `TEXTURE1` and `TEXTURE2` lumps define `name`.
///
/// # Errors
///
/// [`TextureSet::load`]' [`CyberError::Format`] for a malformed texture lump.
fn holds_texture(session: &Session, name: LumpName) -> Result<bool, CyberError> {
    Ok(TextureSet::load(session)?.find(&name.to_string()).is_some())
}

/// The preview file one lump converts to, chosen by [`family`].
///
/// # Errors
///
/// The chosen converter's errors; [`CyberError::Unsupported`] naming the way
/// out for a family nothing owns.
fn convert(view: &View<'_>, row: &LumpRow) -> Result<Prepared, CyberError> {
    match family(row.kind, row.namespace, &row.lump.data) {
        Family::Flat => flat_asset(view, row),
        Family::Picture => patch_asset(view, row),
        Family::Image => image_asset(row),
        Family::Sound => sound_asset(row),
        Family::Music => music_asset(row),
        Family::Palette => palette_asset(view, row),
        Family::Colormap => colormap_asset(view, row),
        Family::Other => Err(unsupported(row)),
    }
}

/// The refusal for bytes no converter owns.
fn unsupported(row: &LumpRow) -> CyberError {
    CyberError::Unsupported(format!(
        "a {} named {} is nothing to look at or listen to; use get_lump format=url for the raw \
         bytes",
        row.kind.as_str(),
        row.name
    ))
}

/// The composite-texture preview: the record's patches drawn onto its own
/// canvas, a patch the view cannot supply leaving a transparent hole, as a
/// `PNG` the requested palette colours.
///
/// # Errors
///
/// [`load_texture`]' errors for the definition, and [`View::palette`]' for
/// the colours.
fn texture_asset(view: &View<'_>, name: LumpName) -> Result<Prepared, CyberError> {
    let composite = load_texture(view.session, name)?;
    indexed_asset(view, "texture", &composite.image, None)
}

/// The flat preview: a raw indexed plane, or the `PNG` a port stores in a
/// flat lump, recoloured.
///
/// # Errors
///
/// [`decode_flat_bytes`]' errors for bytes that are neither, and
/// [`View::palette`]' for the colours.
fn flat_asset(view: &View<'_>, row: &LumpRow) -> Result<Prepared, CyberError> {
    let image = decode_flat_bytes(view.session, &row.lump.data)?;
    indexed_asset(view, "flat", &image, None)
}

/// The patch or sprite preview: the stored picture with its drawing offsets
/// published in a `grAb` chunk.
///
/// # Errors
///
/// [`decode_patch_bytes`]' errors for bytes that are neither a `Doom` patch
/// nor a readable `PNG`, and [`View::palette`]' for the colours.
fn patch_asset(view: &View<'_>, row: &LumpRow) -> Result<Prepared, CyberError> {
    let patch = decode_patch_bytes(view.session, &row.lump.data)?;
    let label = if row.kind == LumpKind::Sprite {
        "sprite"
    } else {
        "patch"
    };
    indexed_asset(view, label, &patch.image, Some(grab_of(&patch)))
}

/// One patch's drawing offsets, as a `grAb` chunk stores them.
fn grab_of(patch: &Patch) -> GrabOffsets {
    (i32::from(patch.left_offset), i32::from(patch.top_offset))
}

/// The embedded-picture preview: a `PNG`, `JPEG` or `GIF` lump re-encoded as
/// `PNG`, keeping the `grAb` offsets a `PNG` carries.
///
/// # Errors
///
/// [`decode_image`]' [`CyberError::Format`] for bytes no decoder reads;
/// [`read_grab`]' for a chunk stream that does not parse.
fn image_asset(row: &LumpRow) -> Result<Prepared, CyberError> {
    let data: &[u8] = &row.lump.data;
    let image = decode_image(data)?;
    Ok(Prepared {
        resolved_as: "image",
        extension: "png",
        data: encode_png(&image, embedded_grab(data)?)?,
        size: Some((image.width, image.height)),
    })
}

/// The offsets an embedded picture carries: a `PNG`'s `grAb` chunk, and
/// nothing for the formats that have no such thing.
///
/// # Errors
///
/// [`read_grab`]' [`CyberError::Format`] for a malformed chunk stream.
fn embedded_grab(data: &[u8]) -> Result<Option<GrabOffsets>, CyberError> {
    if is_png(data) {
        read_grab(data)
    } else {
        Ok(None)
    }
}

/// The sound preview: the same playable file `preview_sound` publishes at
/// its default sample width.
///
/// # Errors
///
/// [`sound_to_playable`]' errors for sound bytes that do not decode.
fn sound_asset(row: &LumpRow) -> Result<Prepared, CyberError> {
    let (data, extension, _info) = sound_to_playable(&row.lump.data, row.kind, DEFAULT_BITS)?;
    Ok(Prepared {
        resolved_as: "sound",
        extension,
        data,
        size: None,
    })
}

/// The music preview: the same playable file `preview_music` publishes.
///
/// # Errors
///
/// [`music_to_playable`]' errors for a score or container it cannot read.
fn music_asset(row: &LumpRow) -> Result<Prepared, CyberError> {
    let (data, extension, _kind) = music_to_playable(&row.lump.data)?;
    Ok(Prepared {
        resolved_as: "music",
        extension,
        data,
        size: None,
    })
}

/// The palette preview: the `256x256` swatch `get_palette` publishes, of the
/// requested entry.
///
/// # Errors
///
/// [`View::palette`]' [`CyberError::NotFound`] and [`RgbaImage::new`]' error
/// for the grid.
fn palette_asset(view: &View<'_>, row: &LumpRow) -> Result<Prepared, CyberError> {
    debug_assert_eq!(row.kind, LumpKind::Palette, "only palettes route here");
    let grid = swatch_image(&view.palette()?)?;
    png_asset("palette", &grid, None)
}

/// The light-table preview: one row per `COLORMAP` table, one column per
/// palette index, drawn through palette entry `0`.
///
/// # Errors
///
/// [`colormap_image`]' errors for the table bytes, and [`load_palette`]'
/// [`CyberError::NotFound`] for a session whose `PLAYPAL` has no entry `0`.
fn colormap_asset(view: &View<'_>, row: &LumpRow) -> Result<Prepared, CyberError> {
    let normal = load_palette(view.session, COLORMAP_PALETTE)?;
    let grid = colormap_image(&row.lump.data, &normal)?;
    png_asset("colormap", &grid, None)
}

/// The `COLORMAP` grid: the tables laid out top to bottom, their mapped
/// indices expanded through `palette` left to right.
///
/// # Errors
///
/// [`CyberError::Format`] when the payload is not a whole number of
/// [`COLORMAP_TABLE`]-byte tables or holds too many of them for an image.
fn colormap_image(data: &[u8], palette: &Palette) -> Result<RgbaImage, CyberError> {
    let tables = data.chunks_exact(COLORMAP_TABLE);
    if !tables.remainder().is_empty() {
        return Err(CyberError::Format(format!(
            "a COLORMAP is whole 256-byte light tables and this lump holds {} bytes; use \
             get_lump format=url for the raw bytes",
            data.len()
        )));
    }
    let mut pixels: Vec<u8> = Vec::with_capacity(data.len().saturating_mul(RGBA_BYTES));
    for entry in data {
        let [red, green, blue] = palette.color(usize::from(*entry)).unwrap_or(BLACK);
        pixels.extend_from_slice(&[red, green, blue, OPAQUE]);
    }
    let height = u32::try_from(tables.count()).map_err(|error| {
        CyberError::LimitExceeded(format!(
            "the COLORMAP's tables do not fit an image height: {error}"
        ))
    })?;
    RgbaImage::new(COLORMAP_COLUMNS, height, pixels)
}

/// The `PNG` asset of one indexed picture, coloured with the view's palette
/// and carrying `offsets` in a `grAb` chunk when it has any.
///
/// # Errors
///
/// [`View::palette`]' and [`encode_png`]' errors.
fn indexed_asset(
    view: &View<'_>,
    label: &'static str,
    image: &IndexedImage,
    offsets: Option<GrabOffsets>,
) -> Result<Prepared, CyberError> {
    png_asset(label, &image.to_rgba(&view.palette()?), offsets)
}

/// The `PNG` asset of one `RGBA` picture.
///
/// # Errors
///
/// [`encode_png`]' errors.
fn png_asset(
    label: &'static str,
    image: &RgbaImage,
    offsets: Option<GrabOffsets>,
) -> Result<Prepared, CyberError> {
    Ok(Prepared {
        resolved_as: label,
        extension: "png",
        data: encode_png(image, offsets)?,
        size: Some((image.width, image.height)),
    })
}

/// Run `get_asset_url` end to end: resolve the name, convert what was found
/// and publish the file.
///
/// # Errors
///
/// [`Request::new`]' errors for the arguments; [`prepare`]' for the session
/// and the converters; [`AppState::publish`]' for the asset store.
fn run(state: &AppState, args: &GetAssetUrlParams) -> Result<AssetUrlReport, CyberError> {
    let request = Request::new(args)?;
    let palette = args.palette.unwrap_or(DEFAULT_PALETTE);
    let prepared = state.with_session(|session| prepare(session, &request, palette))?;
    publish_asset(state, request.name, prepared)
}

/// Stores one prepared file as `NAME.<extension>` and reports it.
///
/// # Errors
///
/// [`AppState::publish`]' errors for the asset store.
fn publish_asset(
    state: &AppState,
    name: LumpName,
    prepared: Prepared,
) -> Result<AssetUrlReport, CyberError> {
    let Prepared {
        resolved_as,
        extension,
        data,
        size,
    } = prepared;
    let asset = state.publish(
        &format!("{name}.{extension}"),
        mime_for_extension(extension),
        data,
    )?;
    Ok(AssetUrlReport {
        name,
        resolved_as,
        width: size.map(|(width, _height)| width),
        height: size.map(|(_width, height)| height),
        mime: asset.mime.clone(),
        asset,
    })
}

#[expect(
    clippy::multiple_inherent_impl,
    reason = "rmcp's documented multi-group layout needs one #[tool_router] impl block per group"
)]
#[tool_router(router = assets_router, vis = "pub(crate)")]
impl Cyberdemon {
    /// Reports the asset server's origin and the store's current fill.
    #[tool(
        description = "Reports the localhost asset server (base URL and upload URL template) plus the store's capacity and current fill, starting the server when it has not run yet. Use it before uploading files a tool should fetch, or to check what the store holds."
    )]
    async fn get_asset_server_info(&self) -> Result<CallToolResult, ErrorData> {
        let state = self.state();
        let base_url = match state.asset_base_url() {
            Ok(url) => url,
            Err(error) => return fail(&error),
        };
        let info = state.with_assets(|store| {
            Ok(AssetServerInfo {
                base_url: base_url.clone(),
                upload_url_template: format!("{base_url}/upload/{{filename}}"),
                capacity: store.capacity(),
                stored: store.len(),
                stored_bytes: store.total_bytes(),
                max_asset_bytes: MAX_ASSET_BYTES,
            })
        });
        match info {
            Ok(value) => ok(&value),
            Err(error) => fail(&error),
        }
    }

    /// Resizes the asset store at runtime.
    #[tool(
        description = "Sets how many of the most recent assets the store keeps (1..=10000) and reports how many the change evicted. Use it when the default 50 keeps too few renders or fills memory; an evicted asset's URL returns 404, so regenerate rather than retry."
    )]
    async fn set_asset_capacity(
        &self,
        Parameters(args): Parameters<SetAssetCapacityParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let report = self.state().with_assets(|store| {
            let evicted = store.set_capacity(args.capacity)?;
            Ok(CapacityReport {
                capacity: args.capacity,
                evicted,
            })
        });
        match report {
            Ok(value) => ok(&value),
            Err(error) => fail(&error),
        }
    }

    /// Publishes any viewable or playable session resource as one asset.
    #[tool(
        description = "Resolves one name - a composite texture, flat, patch, sprite, embedded picture, sound, song, PLAYPAL or COLORMAP - and publishes it as a viewable or playable file on the localhost asset server; nothing is edited. Takes the shared lump selector plus palette and prefer. Use it whenever asked to show or play something by name; the specific get_* tools report richer measurements."
    )]
    fn get_asset_url(
        &self,
        Parameters(args): Parameters<GetAssetUrlParams>,
    ) -> Result<CallToolResult, ErrorData> {
        match run(self.state(), &args) {
            Ok(report) => ok(&report),
            Err(error) => fail(&error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        COLORMAP_TABLE, Family, GetAssetUrlParams, Prefer, Request, colormap_image, embedded_grab,
        family, is_colormap, is_flat, is_music, is_picture, unsupported,
    };
    use crate::error::CyberError;
    use crate::gfx::codec::encode_png;
    use crate::gfx::image::RgbaImage;
    use crate::gfx::palette::Palette;
    use crate::tools::lumps::rows::LumpRow;
    use crate::tools::lumps::select::LumpSelector;
    use crate::wad::archive::Lump;
    use crate::wad::classify::{LumpKind, Namespace};
    use crate::wad::name::LumpName;

    /// The error code of a [`CyberError`], for compact asserts.
    fn code(error: &CyberError) -> &str {
        error.code()
    }

    /// Selector arguments naming `text` and nothing else.
    fn selector(text: &str) -> LumpSelector {
        LumpSelector {
            name: text.to_owned(),
            scope: None,
            namespace: None,
            occurrence: None,
        }
    }

    /// A 14-entry `PLAYPAL` whose colour `i` is `(i, 255 - i, i)`.
    fn palette() -> Palette {
        let mut entry: Vec<u8> = Vec::new();
        for index in 0_u8..=255 {
            entry.extend_from_slice(&[index, 255_u8.wrapping_sub(index), index]);
        }
        let mut playpal = Vec::new();
        for _table in 0_u8..14 {
            playpal.extend_from_slice(&entry);
        }
        Palette::from_playpal(&playpal, 0).expect("the fixture PLAYPAL is valid")
    }

    /// `count` copies of the identity `COLORMAP` table.
    fn colormap(tables: usize) -> Vec<u8> {
        let mut bytes = Vec::new();
        for _table in 0..tables {
            bytes.extend(0_u8..=255);
        }
        bytes
    }

    /// The bytes of a `Doom` patch header of a plausible shape.
    fn patch_header() -> Vec<u8> {
        let mut bytes = Vec::new();
        for field in [2_u16, 2, 0, 0] {
            bytes.extend_from_slice(&field.to_le_bytes());
        }
        bytes.extend_from_slice(&16_i32.to_le_bytes());
        bytes.extend_from_slice(&20_i32.to_le_bytes());
        bytes.resize(240, 0_u8);
        bytes
    }

    /// The kind, region and bytes of a `DEHACKED` text lump.
    fn text_row() -> LumpRow {
        LumpRow {
            index: 0,
            name: LumpName::parse("DEHACKED").expect("a valid name"),
            size: 4,
            offset: 0,
            kind: LumpKind::Dehacked,
            namespace: None,
            map: None,
            source: crate::session::Source::Working,
            lump: Lump::new(
                LumpName::parse("DEHACKED").expect("a valid name"),
                vec![b'a'; 4],
            ),
        }
    }

    #[test]
    fn prefer_accepts_the_two_words_and_defaults_to_the_lump() {
        assert_eq!(Prefer::parse(None).expect("the default"), Prefer::Lump);
        assert_eq!(Prefer::parse(Some("lump")).expect("the lump"), Prefer::Lump);
        assert_eq!(
            Prefer::parse(Some("texture")).expect("the texture"),
            Prefer::Texture
        );
        let error = Prefer::parse(Some("sprites")).expect_err("not a choice");
        assert_eq!(code(&error), "invalid_argument", "bad prefer code");
        assert!(
            error.to_string().contains("TEXTURE1"),
            "the message names the fix: {error}"
        );
    }

    #[test]
    fn the_dispatch_tables_the_picture_families() {
        let patch_bytes = patch_header();
        assert_eq!(
            family(LumpKind::Flat, None, &[9_u8; 4_096]),
            Family::Flat,
            "a classified flat"
        );
        assert_eq!(
            family(LumpKind::Png, Some(Namespace::Flats), b"whatever"),
            Family::Flat,
            "the flats region answers even for a PNG flat"
        );
        assert_eq!(
            family(LumpKind::Patch, None, b"junk"),
            Family::Picture,
            "a patches-region lump"
        );
        assert_eq!(
            family(LumpKind::Unknown, None, &patch_bytes),
            Family::Picture,
            "bytes with a patch header"
        );
        assert_eq!(
            family(LumpKind::Sprite, None, b"junk"),
            Family::Picture,
            "a sprites-region lump"
        );
    }

    #[test]
    fn the_dispatch_tables_the_media_and_the_leftovers() {
        assert_eq!(
            family(LumpKind::Jpeg, None, b"\xFF\xD8\xFF"),
            Family::Image,
            "an embedded JPEG"
        );
        assert_eq!(
            family(LumpKind::DoomSound, None, b"\x03\x00\x00\x00\x01\x00"),
            Family::Sound,
            "a DMX sound"
        );
        assert_eq!(
            family(LumpKind::Mus, None, b"MUS\x1A"),
            Family::Music,
            "MUS"
        );
        assert_eq!(
            family(LumpKind::Unknown, None, b"Extended Module: rest"),
            Family::Music,
            "a tracker module sniffs its way in"
        );
        assert_eq!(
            family(LumpKind::Palette, None, b""),
            Family::Palette,
            "PLAYPAL"
        );
        assert_eq!(
            family(LumpKind::Unknown, Some(Namespace::Colormaps), b"maps"),
            Family::Colormap,
            "the colormaps region"
        );
        assert_eq!(
            family(LumpKind::Dehacked, None, b"text"),
            Family::Other,
            "a text lump is nothing to look at"
        );
    }

    #[test]
    fn the_family_predicates_answer_for_the_boundaries() {
        assert!(is_flat(LumpKind::Flat, None), "the classifier's verdict");
        assert!(
            is_flat(LumpKind::Unknown, Some(Namespace::Flats)),
            "the region's verdict"
        );
        assert!(
            !is_flat(LumpKind::Patch, Some(Namespace::Patches)),
            "not a flat"
        );
        assert!(
            is_picture(LumpKind::Sprite, b"junk"),
            "a sprite region lump"
        );
        assert!(
            !is_picture(LumpKind::Unknown, b"junk"),
            "plain bytes are no patch"
        );
        assert!(is_music(LumpKind::Midi, b"MThd"), "a MIDI lump");
        assert!(!is_music(LumpKind::Text, b"words"), "text is no music");
        assert!(
            is_colormap(LumpKind::Colormap, None),
            "the classified table"
        );
        assert!(
            is_colormap(LumpKind::Unknown, Some(Namespace::Colormaps)),
            "the region"
        );
        assert!(
            !is_colormap(LumpKind::Patch, None),
            "a patch is no colormap"
        );
    }

    #[test]
    fn embedded_grab_reads_a_png_and_shrugs_at_the_other_formats() {
        let img = RgbaImage::new(1, 1, vec![0_u8; 4]).expect("1x1 is valid");
        let png = encode_png(&img, Some((3_i32, -4_i32))).expect("the fixture PNG encodes");
        assert_eq!(
            embedded_grab(&png).expect("a chunked PNG parses"),
            Some((3_i32, -4_i32)),
            "a grAb chunk survives"
        );
        assert_eq!(
            embedded_grab(b"\xFF\xD8\xFF\xE0junk").expect("no chunk to read"),
            None,
            "a JPEG has nothing to keep"
        );
        let broken = vec![0x89_u8, b'P', b'N', b'G', b'\r', b'\n', 0x1A, b'\n'];
        assert_eq!(
            code(&embedded_grab(&broken).expect_err("no chunks at all")),
            "format_error",
            "a truncated PNG says so"
        );
    }

    #[test]
    fn the_colormap_grid_is_one_row_per_table_and_one_column_per_entry() {
        let grid = colormap_image(&colormap(34), &palette()).expect("34 tables make a grid");
        assert_eq!((grid.width, grid.height), (256, 34), "the documented shape");
        assert_eq!(
            grid.get(0, 0),
            Some([0_u8, 255, 0, 255]),
            "the identity table's entry 0"
        );
        assert_eq!(
            grid.get(17, 33),
            Some([17_u8, 238, 17, 255]),
            "the last table's entry 17"
        );
    }

    #[test]
    fn a_colormap_that_is_not_whole_tables_is_a_format_error() {
        let torn = COLORMAP_TABLE.saturating_mul(2).saturating_sub(1);
        let data = colormap(2);
        let error = colormap_image(data.get(..torn).unwrap_or(&data), &palette())
            .expect_err("a torn table set");
        assert_eq!(code(&error), "format_error", "torn colormap code");
        assert!(
            error.to_string().contains("get_lump format=url"),
            "the message names the raw way out: {error}"
        );
    }

    #[test]
    fn an_unconvertible_lump_is_refused_with_the_raw_bytes_way_out() {
        let error = unsupported(&text_row());
        assert_eq!(code(&error), "unsupported", "a text lump code");
        let message = error.to_string();
        assert!(
            message.contains("dehacked") && message.contains("DEHACKED"),
            "the message names the kind and the lump: {message}"
        );
        assert!(
            message.contains("get_lump format=url"),
            "and the way out: {message}"
        );
    }

    #[test]
    fn a_request_normalises_its_name_and_choice() {
        let args = GetAssetUrlParams {
            selector: selector("wall1"),
            palette: None,
            prefer: None,
        };
        let request = Request::new(&args).expect("a valid call");
        assert_eq!(request.name.to_string(), "WALL1", "the name is uppercased");
        assert_eq!(request.prefer, Prefer::Lump, "the default choice");
        let long = GetAssetUrlParams {
            selector: selector("TOO MANY CHARACTERS"),
            palette: Some(1),
            prefer: Some("texture".to_owned()),
        };
        assert_eq!(
            code(&Request::new(&long).expect_err("nine characters")),
            "invalid_argument",
            "an impossible name is refused before the session is read"
        );
    }
}
