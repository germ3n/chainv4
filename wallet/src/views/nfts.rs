//! What the account holds of the chain's collections (docs/nft-standard.md): every collection the
//! registry lists is asked what each of the account's addresses holds, with tokens_of, and each NFT
//! shows once the light client proved its owner, or a multi-token amount, against a header it
//! holds. What a token is called and looks like lives where its token_uri points: a data: URI on
//! the chain shows always, one on a server or IPFS only once Settings lets the wallet fetch it, as
//! each fetch tells whoever serves it what this wallet looks at. Sending one is a call the Send
//! page reviews and signs: safe_transfer to a contract, which must say it takes it, transfer to a
//! key.

use {
    super::send::{Prepared, SendState},
    super::{Ctx, Page},
    crate::chain::Held,
    crate::format,
    crate::node::Ask,
    crate::theme,
    chainv4::crypto::hasher::Hasher,
    chainv4::nft::{Located, MOST_BYTES, Metadata, locate},
    chainv4::registry::collections::CollectionKind,
    chainv4::structs::address::Address,
    chainv4::structs::amount::Amount,
    chainv4::structs::hash::HashType,
    chainv4::structs::tx::{Tx, TxCallArgumentType},
    chainv4::structs::u256::u256,
    egui::{RichText, Ui},
    serde_json::{Value, json},
    std::collections::HashMap,
};

/// How often the collections, and what the account holds of them, are asked again.
const ASK_EVERY: f64 = 20.0;
/// The most ids asked of a collection for one address: tokens_of's page.
const PAGE: u64 = 100;
/// The side of a picture in the grid.
const SIDE: f32 = 148.0;

/// A collection as the registry lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listed {
    pub address: String,
    pub kind: CollectionKind,
    pub name: Option<String>,
    pub symbol: Option<String>,
}

impl Listed {
    /// Its name, else its symbol, else its address, short.
    fn title(&self) -> String {
        self.name.clone().or_else(|| self.symbol.clone()).unwrap_or_else(|| format::short(&self.address))
    }
}

/// What came of a URI: still on its way, read, not shown and why.
#[derive(Clone)]
enum Media {
    Asked,
    Metadata(Metadata),
    Picture(egui::ColorImage),
    Shown(egui::TextureHandle),
    Not(String),
}

#[derive(Default)]
pub struct NftsState {
    collections: Vec<Listed>,
    /// The registry's pages so far, while it comes a page at a time, and the cursor of the next.
    asked: f64,
    /// The ids each address holds of each collection, as tokens_of said: where to look, the proof
    /// says what is so.
    held: HashMap<(String, String), Vec<String>>,
    /// Each token's URI, by collection and id.
    uris: HashMap<(String, String), String>,
    /// What each URI came to, by the URI.
    media: HashMap<String, Media>,
    /// The NFT open, by collection and id, with the address holding it.
    open: Option<(String, String, String)>,
    /// The account's addresses, as the page last drew: whose amounts of each multi-token id to prove.
    addresses: Vec<String>,
    /// Why tokens_of was not answered for an address, by its text, until it is.
    unasked: HashMap<String, String>,
    to: String,
    amount: String,
    trouble: Option<String>,
}

/// The collections a /collections answer lists.
pub fn listed(value: &Value) -> Vec<Listed> {
    value["collections"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|found| {
            let kind = match found["kind"].as_str()? {
                "nft" => CollectionKind::Nft,
                "multi" => CollectionKind::Multi,
                _ => return None,
            };
            let text = |key: &str| found[key].as_str().map(str::to_string);

            Some(Listed { address: text("address")?, kind, name: text("name"), symbol: text("symbol") })
        })
        .collect()
}

fn call_ask(contract: &str, function: &str, args: Value) -> Ask {
    Ask::Call(json!({ "contract": contract, "function": function, "args": args, "purpose": "nfts" }).to_string())
}

fn values(result: &Result<Value, crate::node::Failure>) -> Option<Vec<String>> {
    let value = result.as_ref().ok().filter(|value| value["status"] == "returned")?;
    let text = |value: &Value| value["value"].as_str().map_or_else(|| value["value"].to_string(), str::to_string);

    Some(value["values"].as_array()?.iter().map(text).collect())
}

impl NftsState {
    /// Takes the collection registry's entries, as the light client's whole listing of it holds
    /// them: the chain asks it a page at a time, for its proofs and this page both.
    pub fn collections(&mut self, entries: &[Value]) {
        self.collections = listed(&serde_json::json!({ "collections": entries }));
    }

    /// Takes the collections' answers; false for anyone else's.
    pub fn answered(&mut self, ask: &Ask, result: &Result<Value, crate::node::Failure>) -> bool {
        let Ask::Call(body) = ask else {
            return false;
        };
        let Ok(body) = serde_json::from_str::<Value>(body) else {
            return false;
        };

        if body["purpose"] != "nfts" {
            return false;
        }

        let contract = body["contract"].as_str().unwrap_or_default().to_string();
        let first = body["args"][0]["value"].as_str().unwrap_or_default().to_string();

        match (body["function"].as_str(), values(result)) {
            (Some("tokens_of"), Some(ids)) => {
                self.unasked.remove(&first);
                self.held.insert((contract, first), ids);
            }
            // What it held before stands until it is answered: a list lost on the way must not take
            // an NFT off the page, nor leave the page saying nothing of it.
            (Some("tokens_of"), None) => {
                let why = match result {
                    Ok(value) => value["error"].as_str().unwrap_or("the collection would not say").to_string(),
                    Err(failure) => failure.message.clone(),
                };
                self.unasked.insert(first, why);
            }
            (Some("token_uri"), Some(uri)) => {
                if let Some(uri) = uri.into_iter().next() {
                    self.uris.insert((contract, first), uri);
                }
            }
            _ => {}
        }

        true
    }

    /// Takes what was fetched for a URI: metadata, or a picture.
    pub fn fetched(&mut self, url: &str, result: Result<Vec<u8>, String>, gateway: &str) {
        let uris: Vec<String> = self.media.keys().filter(|uri| matches!(locate(uri, gateway), Some(Located::Fetch(at)) if at == url)).cloned().collect();

        for uri in uris {
            let read = match &result {
                Ok(bytes) => read(bytes, None),
                Err(why) => Media::Not(why.clone()),
            };
            self.media.insert(uri, read);
        }
    }

    /// What the page wants proven: the owner of each NFT any of the account's addresses was listed
    /// as holding, or each of its addresses' amount of each multi-token id any of them was. The
    /// proof says where it is now, which may be another of the account's addresses than the list
    /// said, as when it was sent from one to another since.
    pub fn wanted(&self) -> Vec<Held> {
        let mut wanted = Vec::new();

        for listed in &self.collections {
            for id in self.ids(listed) {
                match listed.kind {
                    CollectionKind::Nft => wanted.push(Held { collection: listed.address.clone(), id, multi: false, holder: String::new() }),
                    CollectionKind::Multi => {
                        for holder in &self.addresses {
                            wanted.push(Held { collection: listed.address.clone(), id: id.clone(), multi: true, holder: holder.clone() });
                        }
                    }
                }
            }
        }

        wanted
    }

    /// The ids of a collection any of the account's addresses was listed as holding, each once.
    fn ids(&self, listed: &Listed) -> Vec<String> {
        let mut ids: Vec<String> = Vec::new();

        for holder in &self.addresses {
            for id in self.held.get(&(listed.address.clone(), holder.clone())).into_iter().flatten() {
                if !ids.contains(id) {
                    ids.push(id.clone());
                }
            }
        }

        ids
    }

    /// Keeps each card's id listed under the address the proof found it at, so it stays followed
    /// there until that address's own list says otherwise.
    fn follow(&mut self, cards: &[Card]) {
        for card in cards {
            let listed = self.held.entry((card.listed.address.clone(), card.holder.clone())).or_default();

            if !listed.contains(&card.id) {
                listed.push(card.id.clone());
            }
        }
    }
}

/// Bytes a URI led to: metadata, or a picture, or why they are neither. A media type, when the
/// URI carried one, says which to try.
fn read(bytes: &[u8], media: Option<&str>) -> Media {
    if media.is_none_or(|media| media.contains("json"))
        && let Some(metadata) = Metadata::read(bytes)
    {
        return Media::Metadata(metadata);
    }

    if media.is_some_and(|media| media.contains("svg")) || bytes.trim_ascii_start().starts_with(b"<") {
        return Media::Not("an SVG picture, which the wallet does not draw".to_string());
    }

    match image::load_from_memory(bytes) {
        Ok(picture) => {
            // Big ones shrink to what a card shows; small ones stay as they are.
            let picture = match picture.width().max(picture.height()) > 512 {
                true => picture.thumbnail(512, 512).to_rgba8(),
                false => picture.to_rgba8(),
            };
            let size = [picture.width() as usize, picture.height() as usize];

            Media::Picture(egui::ColorImage::from_rgba_unmultiplied(size, picture.as_raw()))
        }
        Err(_) => Media::Not("neither metadata nor a picture the wallet reads: PNG, JPEG or WebP".to_string()),
    }
}

/// Starts on a URI: data: read in place, anything else fetched once Settings lets it be.
fn want(ctx: &mut Ctx, state: &mut NftsState, uri: &str) {
    if state.media.contains_key(uri) {
        return;
    }

    let media = match locate(uri, ctx.settings.ipfs_gateway.trim()) {
        Some(Located::Inline { media, bytes }) => read(&bytes, Some(&media)),
        Some(Located::Fetch(_)) if !ctx.settings.nft_media => return,
        Some(Located::Fetch(url)) => {
            ctx.node.fetch(&url, MOST_BYTES);
            Media::Asked
        }
        None => Media::Not("a URI the wallet does not follow".to_string()),
    };

    state.media.insert(uri.to_string(), media);
}

fn refresh(ctx: &mut Ctx, state: &mut NftsState, time: f64) {
    state.addresses = ctx.addresses();

    if time - state.asked < ASK_EVERY {
        return;
    }

    // The registry's collections come from the light client's listing of it, which it proves.
    state.asked = time;

    for listed in state.collections.clone() {
        for address in ctx.addresses() {
            let args = json!([
                { "type": "address", "value": address },
                { "type": "u256", "value": "0" },
                { "type": "u256", "value": PAGE.to_string() },
            ]);
            ctx.node.ask(call_ask(&listed.address, "tokens_of", args));
        }
    }

    // Each token's URI, once.
    let held: Vec<(String, String)> = state.held.iter().flat_map(|((collection, _), ids)| ids.iter().map(move |id| (collection.clone(), id.clone()))).collect();

    for (collection, id) in held {
        if !state.uris.contains_key(&(collection.clone(), id.clone())) {
            ctx.node.ask(call_ask(&collection, "token_uri", json!([{ "type": "u256", "value": id }])));
        }
    }
}

/// One NFT as a row of the grid shows it: what the light client proved of it for this holder.
struct Card {
    listed: Listed,
    id: String,
    holder: String,
    /// How many: 1 for an NFT.
    amount: u256,
    assurance: chainv4::light::Assurance,
}

/// The NFTs proven held by the account's addresses: each at the address its proof names, whichever
/// of them listed it, so one sent between the account's own addresses shows at the one it went to.
/// The owner proven is compared as an address, not as text.
fn cards(ctx: &Ctx, state: &NftsState) -> Vec<Card> {
    let mine = ctx.holder().map(|holder| holder.addresses()).unwrap_or_default();
    let mut cards = Vec::new();

    for listed in &state.collections {
        for id in state.ids(listed) {
            match listed.kind {
                CollectionKind::Nft => {
                    let owned = ctx.chain.nft_owners.get(&(listed.address.clone(), id.clone())).and_then(|proven| Some((proven.value.filter(|owner| mine.contains(owner))?, proven.assurance)));

                    if let Some((owner, assurance)) = owned {
                        cards.push(Card { listed: listed.clone(), id, holder: owner.to_string(), amount: u256::from(1u64), assurance });
                    }
                }
                CollectionKind::Multi => {
                    for holder in &mine {
                        let held = ctx.chain.multi_balances.get(&(listed.address.clone(), id.clone(), holder.to_string())).filter(|proven| proven.value > u256::ZERO);

                        if let Some(proven) = held {
                            cards.push(Card { listed: listed.clone(), id: id.clone(), holder: holder.to_string(), amount: proven.value, assurance: proven.assurance });
                        }
                    }
                }
            }
        }
    }

    cards
}

pub fn show(ui: &mut Ui, ctx: &mut Ctx, state: &mut NftsState, send: &mut SendState) {
    let palette = *ctx.palette;
    theme::title(ui, &palette, "NFTs", "What this account holds of the chain's collections, each one proven held against a header the wallet checked.");

    let time = ui.input(|input| input.time);
    refresh(ctx, state, time);

    if !ctx.chain.has_followed() {
        theme::card(ui, &palette, |ui| super::waiting(ui, &palette, "Following the chain first, to prove what the account holds."));

        return;
    }

    let cards = cards(ctx, state);
    state.follow(&cards);
    let waiting = cards.is_empty() && !state.wanted().is_empty();

    for card in &cards {
        if let Some(uri) = state.uris.get(&(card.listed.address.clone(), card.id.clone())).cloned() {
            want(ctx, state, &uri);

            if let Some(Media::Metadata(metadata)) = state.media.get(&uri)
                && let Some(image) = metadata.image.clone()
            {
                want(ctx, state, &image);
            }
        }
    }

    let mut prepared = None;

    if let Some((collection, id, holder)) = state.open.clone() {
        match cards.iter().find(|card| (&card.listed.address, &card.id, &card.holder) == (&collection, &id, &holder)) {
            Some(card) => {
                opened(ui, ctx, state, card, &mut prepared);
            }
            None => state.open = None,
        }
    }

    theme::card(ui, &palette, |ui| {
        theme::caps(ui, &palette, "Held");

        // What a list that did not come leaves out is not known to be gone: say whose.
        for (address, why) in state.unasked.iter().filter(|(address, _)| state.addresses.contains(address)) {
            let label = ctx.holder().and_then(|holder| holder.addresses().into_iter().find(|mine| mine.to_string() == *address)).map_or("An address", |mine| super::scheme(&mine));
            ui.add(egui::Label::new(RichText::new(format!("{label} {}: what it holds was not listed ({why}). Asked again in a moment.", format::short(address))).small().color(palette.warn)).wrap());
        }

        if cards.is_empty() {
            match (state.collections.is_empty(), waiting) {
                (_, true) => super::waiting(ui, &palette, "Proving what the account holds."),
                (true, false) => {
                    ui.label(RichText::new("The chain lists no collections, or the node has not said yet.").color(palette.muted));
                }
                (false, false) => {
                    ui.label(RichText::new(format!("None of the {} collections the chain lists has anything of this account's.", state.collections.len())).color(palette.muted));
                }
            }

            return;
        }

        if !ctx.settings.nft_media {
            ui.label(RichText::new("Pictures and names that live off the chain show once Settings lets the wallet fetch them.").small().color(palette.muted));
            ui.add_space(6.0);
        }

        let gap = ui.spacing().item_spacing.x;
        let (columns, side) = chainv4_ui::layout::tiles(ui.available_width(), SIDE, gap);

        for row in cards.chunks(columns) {
            ui.horizontal_top(|ui| {
                for card in row {
                    if tile(ui, &palette, state, card, side).clicked() {
                        state.open = Some((card.listed.address.clone(), card.id.clone(), card.holder.clone()));
                        state.to.clear();
                        state.amount = "1".to_string();
                        state.trouble = None;
                    }
                }
            });
        }
    });

    if let Some(prepared) = prepared {
        // Back from sending, the lists are asked again at once: the NFT has moved.
        state.asked = f64::NEG_INFINITY;
        send.prepare(prepared);
        *ctx.page = Page::Send;
    }
}

/// What a token's metadata says of it, once read.
fn metadata<'a>(state: &'a NftsState, card: &Card) -> Option<&'a Metadata> {
    match state.media.get(state.uris.get(&(card.listed.address.clone(), card.id.clone()))?) {
        Some(Media::Metadata(metadata)) => Some(metadata),
        _ => None,
    }
}

/// The picture a token shows: its metadata's image, or what its URI is itself.
fn picture(ui: &Ui, state: &mut NftsState, card: &Card) -> Option<egui::TextureHandle> {
    let uri = state.uris.get(&(card.listed.address.clone(), card.id.clone()))?.clone();
    let source = metadata(state, card).and_then(|metadata| metadata.image.clone()).unwrap_or(uri);

    match state.media.get(&source)? {
        Media::Shown(texture) => Some(texture.clone()),
        Media::Picture(image) => {
            let texture = ui.ctx().load_texture(format!("nft {source}"), image.clone(), egui::TextureOptions::LINEAR);
            state.media.insert(source, Media::Shown(texture.clone()));

            Some(texture)
        }
        _ => None,
    }
}

fn title(state: &NftsState, card: &Card) -> String {
    metadata(state, card).and_then(|metadata| metadata.name.clone()).unwrap_or_else(|| format!("{} #{}", card.listed.title(), card.id))
}

/// One token in the grid: its picture and two lines under it, each cut short, so that every tile of
/// a row has the same top and the same height, whatever a token is called.
fn tile(ui: &mut Ui, palette: &theme::Palette, state: &mut NftsState, card: &Card, side: f32) -> egui::Response {
    let texture = picture(ui, state, card);
    let name = title(state, card);

    ui.allocate_ui_with_layout(egui::vec2(side, side), egui::Layout::top_down(egui::Align::Min), |ui| {
        ui.set_width(side);
        let (rect, response) = ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::click());
        ui.painter().rect_filled(rect, 10.0, palette.row);

        match texture {
            Some(texture) => {
                let size = texture.size_vec2();
                let scale = (side / size.x).min(side / size.y);
                let shown = egui::Rect::from_center_size(rect.center(), size * scale);
                ui.painter().image(texture.id(), shown, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
            }
            None => {
                ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, format!("#{}", card.id), egui::FontId::proportional(22.0), palette.muted);
            }
        }

        ui.add(egui::Label::new(RichText::new(&name).strong().color(palette.ink)).truncate());
        let held = match card.listed.kind {
            CollectionKind::Nft => card.listed.title(),
            CollectionKind::Multi => format!("{} × {}", card.amount, card.listed.title()),
        };
        ui.add(egui::Label::new(RichText::new(held).small().color(palette.muted)).truncate());

        response.on_hover_text(format!("{name}\nClick to open"))
    })
    .inner
}

/// An NFT opened: its picture, what its metadata says, where it is proven held, and sending it.
fn opened(ui: &mut Ui, ctx: &mut Ctx, state: &mut NftsState, card: &Card, prepared: &mut Option<Prepared>) {
    let palette = *ctx.palette;
    let texture = picture(ui, state, card);
    let name = title(state, card);
    let uri = state.uris.get(&(card.listed.address.clone(), card.id.clone())).cloned();
    let read = metadata(state, card).cloned();

    theme::card(ui, &palette, |ui| {
        theme::end_row(
            ui,
            |ui| {
                theme::caps(ui, &palette, &name);
            },
            |ui| {
                if theme::quiet(ui, &palette, "Close").clicked() {
                    state.open = None;
                }
            },
        );

        if let Some(texture) = texture {
            let size = texture.size_vec2();
            let width = ui.available_width().min(360.0);
            ui.add(egui::Image::new(&texture).fit_to_exact_size(egui::vec2(width, width * size.y / size.x.max(1.0))));
        }

        if let Some(description) = read.as_ref().and_then(|read| read.description.clone()) {
            ui.label(RichText::new(description).color(palette.soft));
        }

        ui.add_space(6.0);
        super::row(ui, &palette, "Collection", |ui| {
            ui.label(card.listed.title());
            super::address(ui, &palette, &card.listed.address);
        });
        super::row(ui, &palette, "Id", |ui| {
            ui.label(theme::mono(card.id.clone()));
        });
        super::row(ui, &palette, "Held", |ui| {
            match card.listed.kind {
                CollectionKind::Nft => ui.label("This one, by"),
                CollectionKind::Multi => ui.label(format!("{} of it, by", card.amount)),
            };
            super::address(ui, &palette, &card.holder);
            super::assurance(ui, &palette, &card.assurance);
        });

        for (trait_type, value) in read.as_ref().map(|read| read.attributes.clone()).unwrap_or_default() {
            super::row(ui, &palette, if trait_type.is_empty() { "Trait" } else { &trait_type }, |ui| {
                ui.label(value);
            });
        }

        if let Some(uri) = &uri {
            super::row(ui, &palette, "Token URI", |ui| {
                ui.label(RichText::new(format::short(uri)).small().color(palette.muted)).on_hover_text(uri);
            });

            if let Some(Media::Not(why)) = state.media.get(uri) {
                ui.label(RichText::new(why).small().color(palette.warn));
            }
        }

        ui.add_space(10.0);
        theme::caps(ui, &palette, "Send");
        super::row(ui, &palette, "To", |ui| {
            ui.add(egui::TextEdit::singleline(&mut state.to).hint_text("an address or a name").desired_width(f32::INFINITY));
        });

        if card.listed.kind == CollectionKind::Multi {
            super::row(ui, &palette, "How many", |ui| {
                ui.add(egui::TextEdit::singleline(&mut state.amount).desired_width(120.0));
                ui.label(RichText::new(format!("of {}", card.amount)).small().color(palette.muted));
            });
        }

        let to = recipient(ctx, &state.to);
        let amount = match card.listed.kind {
            CollectionKind::Nft => Some(u256::from(1u64)),
            CollectionKind::Multi => u256::from_str_radix(state.amount.trim(), 10).ok().filter(|amount| *amount > u256::ZERO && *amount <= card.amount),
        };

        if !state.to.trim().is_empty() && to.is_none() {
            ui.colored_label(palette.bad, "Not an address, nor a name proven to point at one.");
        }

        if card.listed.kind == CollectionKind::Multi && amount.is_none() {
            ui.colored_label(palette.bad, format!("A whole number from 1 to {}.", card.amount));
        }

        if theme::primary(ui, &palette, "Review and send", to.is_some() && amount.is_some()).clicked()
            && let (Some(to), Some(amount)) = (to, amount)
        {
            match (card.holder.parse::<Address>(), card.listed.address.parse::<Address>()) {
                (Ok(from), Ok(collection)) => *prepared = Some(transfer(from, collection, card.listed.kind, &card.id, to, amount)),
                _ => state.trouble = Some("An address that does not read.".to_string()),
            }
        }

        if let Some(trouble) = &state.trouble {
            ui.colored_label(palette.bad, trouble);
        }
    });
}

/// Who to send to: an address, or a name the light client proved points at one.
fn recipient(ctx: &Ctx, text: &str) -> Option<Address> {
    let text = text.trim();

    text.parse().ok().or_else(|| ctx.chain.resolve(text).and_then(|resolved| resolved.target()))
}

/// Sending an NFT, or an amount of a multi-token id: safe_transfer to a contract, which must say it
/// takes it, else transfer.
pub fn transfer(from: Address, collection: Address, kind: CollectionKind, id: &str, to: Address, amount: u256) -> Prepared {
    let function = match to {
        Address::Contract(_) => "safe_transfer",
        _ => "transfer",
    };
    let id = u256::from_str_radix(id, 10).unwrap_or(u256::ZERO);
    let (args, what) = match kind {
        CollectionKind::Nft => (vec![TxCallArgumentType::Address(to), TxCallArgumentType::U256(id)], format!("Send NFT #{id}")),
        CollectionKind::Multi => (
            vec![TxCallArgumentType::Address(to), TxCallArgumentType::U256(id), TxCallArgumentType::U256(amount)],
            format!("Send {amount} of #{id}"),
        ),
    };

    Prepared {
        from,
        tx: Tx::ContractCall { to: collection, func: Hasher::digest(HashType::Xxh3_64, function.as_bytes()), amount: Amount::U128(0), data: args },
        what,
        about: collection.to_string(),
        coin: u256::ZERO,
    }
}

#[cfg(test)]
mod tests;
