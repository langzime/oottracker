use byteorder::BigEndian;

use {
    std::{
        borrow::Borrow,
        fmt,
        future::Future,
        io::{
            self,
            prelude::*,
        },
        pin::Pin,
        sync::Arc,
    },
    async_proto::Protocol,
    byteorder::ByteOrder as _,
    derive_more::From,
    itertools::{
        EitherOrBoth,
        Itertools as _,
    },
    tokio::io::{
        AsyncRead,
        AsyncReadExt as _,
        AsyncWrite,
        AsyncWriteExt as _,
    },
    ootr::Rando,
    crate::{
        save::{
            self,
            Save,
        },
        region::{
            RegionLookup,
            RegionLookupError,
        },
        scene::{
            Scene,
            SceneFlags,
        },
    },
};

pub const SIZE: usize = 0x80_0000;
pub const NUM_RANGES: usize = 4;
pub static RANGES: [u32; NUM_RANGES * 2] = [
    save::ADDR, save::SIZE as u32,
    0x1c8545, 1,
    0x1ca1c8, 4,
    0x1ca1d8, 8,
];

#[derive(Debug, From, Clone)]
pub enum DecodeError {
    Index(u32),
    IndexRange {
        start: u32,
        end: u32,
    },
    Ranges,
    #[from]
    Save(save::DecodeError),
    Size(usize),
    UnexpectedValue {
        offset: u32,
        field: &'static str,
        value: u8,
    },
    UnexpectedValueRange {
        start: u32,
        end: u32,
        field: &'static str,
        value: Vec<u8>,
    },
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Ram {
    pub save: Save,
    pub current_scene_id: u8,
    pub current_scene_switch_flags: u32,
    pub current_scene_chest_flags: u32,
    pub current_scene_room_clear_flags: u32,
}

impl Ram {
    fn new(
        save: &[u8],
        current_scene_id: u8,
        current_scene_switch_flags: &[u8],
        current_scene_chest_flags: &[u8],
        current_scene_room_clear_flags: &[u8],
    ) -> Result<Ram, DecodeError> {
        Ok(Ram {
            save: Save::from_save_data(save)?,
            current_scene_id,
            current_scene_switch_flags: BigEndian::read_u32(current_scene_switch_flags),
            current_scene_chest_flags: BigEndian::read_u32(current_scene_chest_flags),
            current_scene_room_clear_flags: BigEndian::read_u32(current_scene_room_clear_flags),
        })
    }

    pub fn from_range_bufs(ranges: impl IntoIterator<Item = Vec<u8>>) -> Result<Ram, DecodeError> {
        if let Some((
            save,
            current_scene_id,
            current_scene_switch_flags,
            chest_and_room_clear,
        )) = ranges.into_iter().collect_tuple() {
            let current_scene_id = match current_scene_id[..] {
                [current_scene_id] => current_scene_id,
                _ => return Err(DecodeError::Index(RANGES[2])),
            };
            let (chest_flags, room_clear_flags) = chest_and_room_clear.split_at(4);
            Ok(Ram::new(
                &save,
                current_scene_id,
                &current_scene_switch_flags,
                chest_flags,
                room_clear_flags,
            )?)
        } else {
            Err(DecodeError::Ranges)
        }
    }

    pub fn from_ranges<'a, R: Borrow<[u8]> + ?Sized + 'a, I: IntoIterator<Item = &'a R>>(ranges: I) -> Result<Ram, DecodeError> {
        if let Some((
            save,
            &[current_scene_id],
            current_scene_switch_flags,
            chest_and_room_clear,
        )) = ranges.into_iter().map(Borrow::borrow).collect_tuple() {
            let (chest_flags, room_clear_flags) = chest_and_room_clear.split_at(4);
            Ok(Ram::new(
                save,
                current_scene_id,
                current_scene_switch_flags,
                chest_flags,
                room_clear_flags,
            )?)
        } else {
            Err(DecodeError::Ranges)
        }
    }

    /// Converts an *Ocarina of Time* RAM dump into a `Ram`.
    ///
    /// # Panics
    ///
    /// This method may panic if `ram_data` doesn't contain a valid OoT RAM dump.
    pub fn from_bytes(ram_data: &[u8]) -> Result<Ram, DecodeError> {
        if ram_data.len() != SIZE { return Err(DecodeError::Size(ram_data.len())) }
        Ram::from_ranges(RANGES.iter().tuples().map(|(&start, &len)|
            ram_data.get(start as usize..(start + len) as usize).ok_or(DecodeError::IndexRange { start, end: start + len })
        ).try_collect::<_, Vec<_>, _>()?)
    }

    fn to_ranges(&self) -> Vec<Vec<u8>> {
        let mut chest_and_room_clear = Vec::with_capacity(8);
        chest_and_room_clear.extend_from_slice(&self.current_scene_chest_flags.to_be_bytes());
        chest_and_room_clear.extend_from_slice(&self.current_scene_room_clear_flags.to_be_bytes());
        vec![
            self.save.to_save_data(),
            vec![self.current_scene_id],
            self.current_scene_switch_flags.to_be_bytes().into(),
            chest_and_room_clear,
        ]
    }

    pub(crate) fn current_region<R: Rando>(&self, rando: &R) -> Result<RegionLookup, RegionLookupError<R>> { //TODO disambiguate MQ-ness
        Ok(match Scene::current(self).region(rando, self)? {
            RegionLookup::Dungeon(EitherOrBoth::Both(vanilla, mq)) => {
                //TODO auto-disambiguate
                // visibility of MQ-ness per dungeon
                // immediately upon entering: Deku Tree (torch next to web), Jabu Jabus Belly (boulder and 2 cows), Forest Temple (extra skulltulas and no wolfos), Fire Temple (extra small torches and no hammer blocks), Ganons Castle (extra green bubbles), Spirit Temple (extra switch above and to the right of the exit)
                // not immediately but without checks: Ice Cavern (boulder takes a couple seconds to be visible), Gerudo Training Grounds (the different torches in the first room only become visible after approx. 1 roll forward), Bottom of the Well (the first skulltula being replaced with a ReDead is audible from the entrance)
                // requires checks (exits/locations): Dodongos Cavern (must blow up the first mud block to see that the lobby has an additional boulder)
                // unsure: Water Temple (not sure if the tektite on the ledge of the central pillar is still there in MQ, if not that's the first difference), Shadow Temple (the extra boxes are only visible after going through the first fake wall, not sure if that counts as a check)
                RegionLookup::Dungeon(EitherOrBoth::Both(vanilla, mq))
            }
            lookup => lookup,
        })
    }

    /// Returns the scene flags, with flags for the current scene updated properly.
    pub(crate) fn scene_flags(&self) -> SceneFlags {
        let mut flags = self.save.scene_flags;
        if let Some(flags_scene) = flags.get_mut(Scene::current(self)) {
            flags_scene.set_chests(self.current_scene_chest_flags);
            flags_scene.set_switches(self.current_scene_switch_flags);
            flags_scene.set_room_clear(self.current_scene_room_clear_flags);
            //TODO set collectible flags
            //TODO set unused field? (for triforce pieces; might not be stored separately for current scene at all)
            //TODO set visited rooms (if used)
            //TODO set visited floors (if used)
        }
        flags
    }
}

impl From<Save> for Ram {
    fn from(save: Save) -> Ram {
        Ram { save, ..Ram::default() }
    }
}

#[derive(Debug, From, Clone)]
pub enum ReadError {
    #[from]
    Decode(DecodeError),
    Io(Arc<io::Error>),
}

impl From<io::Error> for ReadError {
    fn from(e: io::Error) -> ReadError {
        ReadError::Io(Arc::new(e))
    }
}

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReadError::Decode(e) => write!(f, "{:?}", e),
            ReadError::Io(e) => write!(f, "I/O error: {}", e),
        }
    }
}

impl Protocol for Ram {
    type ReadError = ReadError;

    fn read<'a, R: AsyncRead + Unpin + Send + 'a>(mut stream: R) -> Pin<Box<dyn Future<Output = Result<Ram, ReadError>> + Send + 'a>> {
        Box::pin(async move {
            let mut ranges = Vec::with_capacity(NUM_RANGES);
            for (_, len) in RANGES.iter().copied().tuples() {
                let mut buf = vec![0; len as usize];
                stream.read_exact(&mut buf).await?;
                ranges.push(buf);
            }
            Ok(Ram::from_range_bufs(ranges)?)
        })
    }

    fn write<'a, W: AsyncWrite + Unpin + Send + 'a>(&'a self, mut sink: W) -> Pin<Box<dyn Future<Output = io::Result<()>> + Send + 'a>> {
        Box::pin(async move {
            let ranges = self.to_ranges();
            for range in ranges {
                sink.write_all(&range).await?;
            }
            Ok(())
        })
    }

    fn read_sync<'a>(mut stream: impl Read + 'a) -> Result<Ram, ReadError> {
        let ranges = RANGES.iter().tuples().map(|(_, &len)| {
            let mut buf = vec![0; len as usize];
            stream.read_exact(&mut buf)?;
            Ok::<_, ReadError>(buf)
        }).try_collect::<_, Vec<_>, _>()?;
        Ok(Ram::from_range_bufs(ranges)?)
    }

    fn write_sync<'a>(&self, mut sink: impl Write + 'a) -> io::Result<()> {
        let ranges = self.to_ranges();
        for range in ranges {
            sink.write_all(&range)?;
        }
        Ok(())
    }
}
