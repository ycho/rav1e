#![allow(safe_extern_statics)]

extern crate bitstream_io;
extern crate byteorder;
extern crate clap;
extern crate libc;
extern crate rand;
extern crate y4m;

#[macro_use]
extern crate enum_iterator_derive;

use std::fs::File;
use std::io::prelude::*;
use bitstream_io::{BE, BitWriter};
use byteorder::*;
use clap::{App, Arg};

// for benchmarking purpose
pub mod ec;
pub mod partition;
pub mod plane;
pub mod context;
pub mod transform;
pub mod quantize;
pub mod predict;
pub mod rdo;

use context::*;
use partition::*;
use transform::*;
use quantize::*;
use plane::*;
use predict::*;
use rdo::*;
use std::fmt;

pub struct Frame {
    pub planes: [Plane; 3]
}

impl Frame {
    pub fn new(width: usize, height:usize) -> Frame {
        Frame {
            planes: [
                Plane::new(width, height, 0, 0),
                Plane::new(width/2, height/2, 1, 1),
                Plane::new(width/2, height/2, 1, 1)
            ]
        }
    }
}

pub struct Sequence {
    pub profile: u8
}

impl Sequence {
    pub fn new() -> Sequence {
        Sequence {
            profile: 0
        }
    }
}

pub struct FrameState {
    pub input: Frame,
    pub rec: Frame
}

impl FrameState {
    pub fn new(fi: &FrameInvariants) -> FrameState {
        FrameState {
            input: Frame::new(fi.sb_width*64, fi.sb_height*64),
            rec: Frame::new(fi.sb_width*64, fi.sb_height*64),
        }
    }
}


// Frame Invariants are invariant inside a frame
#[allow(dead_code)]
pub struct FrameInvariants {
    pub qindex: usize,
    pub width: usize,
    pub height: usize,
    pub sb_width: usize,
    pub sb_height: usize,
    pub number: u64,
    pub ftype: FrameType,
    pub show_existing_frame: bool,
}

impl FrameInvariants {
    pub fn new(width: usize, height: usize, qindex: usize) -> FrameInvariants {
        FrameInvariants {
            qindex: qindex,
            width: width,
            height: height,
            sb_width: (width+63)/64,
            sb_height: (height+63)/64,
            number: 0,
            ftype: FrameType::KEY,
            show_existing_frame: false,
        }
    }
}

impl fmt::Display for FrameInvariants{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Frame {} - {}", self.number, self.ftype)
    }
}

#[allow(dead_code,non_camel_case_types)]
#[derive(Debug,PartialEq,EnumIterator)]
pub enum FrameType {
    KEY,
    INTER,
    INTRA_ONLY,
    S,
}

impl fmt::Display for FrameType{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            &FrameType::KEY => write!(f, "Key frame"),
            &FrameType::INTER => write!(f, "Inter frame"),
            &FrameType::INTRA_ONLY => write!(f, "Intra only frame"),
            &FrameType::S => write!(f, "Switching frame"),
        }
    }
}


pub struct EncoderConfig {
    pub input_file: Box<Read>,
    pub output_file: Box<Write>,
    pub rec_file: Option<Box<Write>>,
    pub limit: u64,
    pub quantizer: usize
}

impl EncoderConfig {
    pub fn from_cli() -> EncoderConfig {
        let matches = App::new("rav1e")
            .version("0.1.0")
            .about("AV1 video encoder")
           .arg(Arg::with_name("INPUT")
                .help("Uncompressed YUV4MPEG2 video input")
                .required(true)
                .index(1))
            .arg(Arg::with_name("OUTPUT")
                .help("Compressed AV1 in IVF video output")
                .short("o")
                .long("output")
                .required(true)
                .takes_value(true))
            .arg(Arg::with_name("RECONSTRUCTION")
                .short("r")
                .takes_value(true))
            .arg(Arg::with_name("LIMIT")
                .help("Maximum number of frames to encode")
                .short("l")
                .long("limit")
                .takes_value(true)
                .default_value("0"))
            .arg(Arg::with_name("QP")
                .help("Quantizer (0-255)")
                .long("quantizer")
                .takes_value(true)
                .default_value("100"))
            .get_matches();

        EncoderConfig {
            input_file: match matches.value_of("INPUT").unwrap() {
                "-" => Box::new(std::io::stdin()) as Box<Read>,
                f @ _ => Box::new(File::open(&f).unwrap()) as Box<Read>
            },
            output_file: match matches.value_of("OUTPUT").unwrap() {
                "-" => Box::new(std::io::stdout()) as Box<Write>,
                f @ _ => Box::new(File::create(&f).unwrap()) as Box<Write>
            },
            rec_file: matches.value_of("RECONSTRUCTION").map(|f| {
                Box::new(File::create(&f).unwrap()) as Box<Write>
            }),
            limit: matches.value_of("LIMIT").unwrap().parse().unwrap(),
            quantizer: matches.value_of("QP").unwrap().parse().unwrap()
        }
    }
}

// TODO: possibly just use bitwriter instead of byteorder
pub fn write_ivf_header(output_file: &mut Write, width: usize, height: usize, num: usize, den: usize) {
    output_file.write(b"DKIF").unwrap();
    output_file.write_u16::<LittleEndian>(0).unwrap(); // version
    output_file.write_u16::<LittleEndian>(32).unwrap(); // header length
    output_file.write(b"AV01").unwrap();
    output_file.write_u16::<LittleEndian>(width as u16).unwrap();
    output_file.write_u16::<LittleEndian>(height as u16).unwrap();
    output_file.write_u32::<LittleEndian>(num as u32).unwrap();
    output_file.write_u32::<LittleEndian>(den as u32).unwrap();
    output_file.write_u32::<LittleEndian>(0).unwrap();
    output_file.write_u32::<LittleEndian>(0).unwrap();
}

pub fn write_ivf_frame(output_file: &mut Write, pts: u64, data: &[u8]) {
    output_file.write_u32::<LittleEndian>(data.len() as u32).unwrap();
    output_file.write_u64::<LittleEndian>(pts).unwrap();
    output_file.write(data).unwrap();
}

fn write_uncompressed_header(packet: &mut Write, sequence: &Sequence, fi: &FrameInvariants) -> Result<(), std::io::Error> {
    let mut uch = BitWriter::<BE>::new(packet);
    uch.write(2,2)?; // frame type
    uch.write(2,sequence.profile)?; // profile 0
    if fi.show_existing_frame {
        uch.write_bit(true)?; // show_existing_frame=1
        uch.write(3,0)?; // show last frame
        uch.byte_align()?;
        return Ok(());
    }
    uch.write_bit(false)?; // show_existing_frame=0
    uch.write_bit(false)?; // keyframe
    uch.write_bit(true)?; // show frame
    uch.write_bit(true)?; // error resilient
    uch.write(1,0)?; // don't use frame ids
    //uch.write(8+7,0)?; // frame id
    uch.write(3,0)?; // colorspace
    uch.write(1,0)?; // color range
    uch.write(16,(fi.sb_width*64-1) as u16)?; // width
    uch.write(16,(fi.sb_height*64-1) as u16)?; // height
    uch.write_bit(false)?; // scaling active
    uch.write_bit(false)?; // screen content tools
    uch.write(3,0x0)?; // frame context
    uch.write(6,0)?; // loop filter level
    uch.write(3,0)?; // loop filter sharpness
    uch.write_bit(false)?; // loop filter deltas enabled
    uch.write(8,fi.qindex as u8)?; // qindex
    uch.write_bit(false)?; // y dc delta q
    uch.write_bit(false)?; // uv dc delta q
    uch.write_bit(false)?; // uv ac delta q
    //uch.write_bit(false)?; // using qmatrix
    uch.write_bit(false)?; // segmentation off
    uch.write(2,0)?; // cdef clpf damping
    uch.write(2,0)?; // cdef bits
    for _ in 0..1 {
        uch.write(7,0)?; // cdef y strength
        uch.write(7,0)?; // cdef uv strength
    }
    uch.write_bit(false)?; // no delta q
    uch.write(6,0)?; // no y, u or v loop restoration
    uch.write_bit(false)?; // tx mode select
    uch.write(2,0)?; // only 4x4 transforms
    //uch.write_bit(false)?; // use hybrid pred
    //uch.write_bit(false)?; // use compound pred
    uch.write_bit(true)?; // reduced tx
    if fi.sb_width*64-1 > 256 {
        uch.write(1,0)?; // tile cols
    }
    uch.write(1,0)?; // tile rows
    uch.write_bit(true)?; // loop filter across tiles
    uch.write(2,0)?; // tile_size_bytes
    uch.byte_align()?;
    Ok(())
}

/// Write into `dst` the difference between the 4x4 blocks at `src1` and `src2`
fn diff_4x4(dst: &mut [i16; 16], src1: &PlaneSlice, src2: &PlaneSlice) {
    for j in 0..4 {
        for i in 0..4 {
            dst[j*4 + i] = (src1.p(i, j) as i16) - (src2.p(i, j) as i16);
        }
    }
}

// For a trasnform block,
// predict, transform, quantize, write coefficients to a bitstream,
// dequantize, inverse-transform.
pub fn write_tx_b(fi: &FrameInvariants, fs: &mut FrameState, cw: &mut ContextWriter,
                  p: usize, bo: &BlockOffset, mode: PredictionMode, tx_type: TxType) {
    let stride = fs.input.planes[p].cfg.stride;
    let rec = &mut fs.rec.planes[p];
    let po = bo.plane_offset(&fs.input.planes[p].cfg);

    if !cw.bc.at(&bo).is_inter() {
        mode.predict_4x4(&mut rec.mut_slice(&po));
    }
    let mut residual = [0 as i16; 16];

    // for debugging
    let ydec = fs.input.planes[p].cfg.ydec;
    if po.y * stride + po.x >= fi.sb_height * (64 >> ydec) * stride {
        let will_crash = 1;
    }

    if (po.y + 3) * stride + po.x + 3 >= fi.sb_height * (64 >> ydec) * stride {
        let will_crash = 1;
    }

    diff_4x4(&mut residual,
             &fs.input.planes[p].slice(&po),
             &rec.slice(&po));

    let mut coeffs = [0 as i32; 16];
    fht4x4(&residual, &mut coeffs, 4, tx_type);
    quantize_in_place(fi.qindex, &mut coeffs);
    cw.write_coeffs(p, bo, &coeffs, TxSize::TX_4X4, tx_type);

    //reconstruct
    let mut rcoeffs = [0 as i32; 16];
    dequantize(fi.qindex, &coeffs, &mut rcoeffs);

    iht4x4_add(&mut rcoeffs, &mut rec.mut_slice(&po).as_mut_slice(), stride, tx_type);
}

fn write_b(fi: &FrameInvariants, fs: &mut FrameState, cw: &mut ContextWriter,
            mode: PredictionMode, bsize: BlockSize, bo: &BlockOffset) {
    cw.bc.at(&bo).mode = mode;
    cw.write_skip(&bo, false);
    cw.write_intra_mode_kf(&bo, mode);
    // FIXME(you): inter mode block does not use uv_mode
    let uv_mode = mode;
    cw.write_intra_uv_mode(uv_mode, mode);
    let tx_type = TxType::DCT_DCT;
    cw.write_tx_type(tx_type, mode);

    let bw = mi_size_wide[bsize as usize];
    let bh = mi_size_high[bsize as usize];

    // FIXME(you): Loop for TX blocks. For now, fixed as a 4x4 TX only,
    // but consider factor out as write_tx_blocks()
    for p in 0..1 {
        for by in 0..bh {
            for bx in 0..bw {
                let tx_bo = BlockOffset{x: bo.x + bx as usize, y: bo.y + by as usize};
                write_tx_b(fi, fs, cw, p, &tx_bo, mode, tx_type);
            }
        }
    }
    let uv_tx_type = exported_intra_mode_to_tx_type_context[uv_mode as usize];
    let uv_bo = BlockOffset{ x: bo.x >> fs.input.planes[1].cfg.xdec,
                            y: bo.x >> fs.input.planes[1].cfg.ydec };
    for p in 1..3 {
        for by in 0..bh >> 1 {
            for bx in 0..bw >> 1 {
                let tx_bo = BlockOffset{x: uv_bo.x + bx as usize, y: uv_bo.y + by as usize};
                write_tx_b(fi, fs, cw, p, &tx_bo, uv_mode, uv_tx_type);
            }
        }
    }
}

// Find the best mode of an predictoin block based on RDO
fn search_best_mode(fi: &FrameInvariants, fs: &mut FrameState,
                  cw: &mut ContextWriter,
                  bsize: BlockSize, bo: &BlockOffset) -> RDOOutput {
    let q = dc_q(fi.qindex) as f64;
    // Lambda formula from doc/theoretical_results.lyx in the daala repo
    let lambda = q*q*2.0_f64.log2()/6.0;

    let mut best_mode = PredictionMode::DC_PRED;
    let mut best_rd = std::f64::MAX;
    let tell = cw.w.tell_frac();
    let w = block_size_wide[bsize as usize];
    let h = block_size_high[bsize as usize];

    for &mode in RAV1E_INTRA_MODES {
        let checkpoint = cw.checkpoint();

        write_b(fi, fs, cw, mode, bsize, bo);
        let po = bo.plane_offset(&fs.input.planes[0].cfg);
        let d = sse_wxh(&fs.input.planes[0].slice(&po), &fs.rec.planes[0].slice(&po),
                        w as usize, h as usize);
        let r = ((cw.w.tell_frac() - tell) as f64)/8.0;

        let rd = (d as f64) + lambda*r;
        if rd < best_rd {
            best_rd = rd;
            best_mode = mode;
        }

        cw.rollback(checkpoint.clone());
    }

    assert!(best_rd as i64 >= 0);

    let rdo_output = RDOOutput { rd_cost: best_rd as u64,
                                pred_mode: best_mode};
    rdo_output
}

// Decide best partition type, recursively.
fn search_partition(fi: &FrameInvariants, fs: &mut FrameState,
                  cw: &mut ContextWriter,
                  bsize: BlockSize, bo: &BlockOffset) -> u64{

    // Partition a block with different partitoin types
    let mut best_partition = PartitionType::PARTITION_NONE;
    let bs = mi_size_wide[bsize as usize];
    let hbs = bs >> 1; // Half the block size in blocks

    // PARITION_NONE
    let rdo_none = search_best_mode(fi, fs, cw, bsize, bo);
    cw.bc.set_mode(bo, rdo_none.pred_mode);

    let mut best_rd_cost = rdo_none.rd_cost;

    let square_blk = mi_size_wide[bsize as usize] == mi_size_high[bsize as usize];

    //let min_splitable_bsize = BlockSize::BLOCK_8X8;
    let min_splitable_bsize = BlockSize::BLOCK_64X64;	//for debugging

    if square_blk && bsize >= min_splitable_bsize {
        let checkpoint = cw.checkpoint();
        let subsize = get_subsize(bsize, PartitionType::PARTITION_SPLIT);

        // PARTITION_SPLIT
        // Split into four quarters.
        // Only place where partition is called recursively.
        let rd_cost0 = search_partition(fi, fs, cw, subsize, bo);
        let rd_cost1 = search_partition(fi, fs, cw, subsize,
                                 &BlockOffset{x: bo.x + hbs as usize, y: bo.y});
        let rd_cost2 = search_partition(fi, fs, cw, subsize,
                                 &BlockOffset{x: bo.x, y: bo.y + hbs as usize});
        let rd_cost3 = search_partition(fi, fs, cw, subsize,
                                 &BlockOffset{x: bo.x + hbs as usize, y: bo.y + hbs as usize});

        cw.rollback(checkpoint.clone());

        let rd_cost_sum = rd_cost0 + rd_cost1 + rd_cost2 + rd_cost3;

        if rd_cost_sum < best_rd_cost {
            best_rd_cost = rd_cost_sum;
            best_partition = PartitionType::PARTITION_SPLIT;
        } else {
            cw.bc.set_mode(bo, rdo_none.pred_mode);
        }

        // TODO(you): More partition types, hor and ver splits first
        // then, more luxurious brand new six typs
        // PARTITION_HOR
        // let rdo0 = search_best_mode(fi, fs, cw, bsize, ...);
        // let rdo1 = search_best_mode(fi, fs, cw, bsize, ...);
        // rd_cost_sum = rdo0.rd_cost + rdo1.rd_cost;


        // PARTITION_VER
        // ...
    }

    cw.bc.set_partition(bo, best_partition);

    // reconstruct with the decided mode
    write_sb(fi, fs, cw, bsize, bo);

    // TODO(you): Consider adding partition cost to best_rd_cost
    best_rd_cost
}

fn write_sb(fi: &FrameInvariants, fs: &mut FrameState, cw: &mut ContextWriter,
            bsize: BlockSize, bo: &BlockOffset) {

    assert!(bsize >= BlockSize::BLOCK_8X8);
    assert!(mi_size_wide[bsize as usize] == mi_size_high[bsize as usize]);

    let partition = cw.bc.get_partition(bo);
    assert!(PartitionType::PARTITION_NONE <= partition &&
            partition < PartitionType::PARTITION_INVALID);

    let bs = mi_size_wide[bsize as usize];
    let hbs = bs >> 1; // Half the block size in blocks
    let subsize = get_subsize(bsize, PartitionType::PARTITION_SPLIT);

    cw.write_partition(bo, partition, bsize);

    match partition {
        PartitionType::PARTITION_NONE => {
            let mode = cw.bc.get_mode(bo);
            write_b(fi, fs, cw, mode, bsize, bo);
        },
        PartitionType::PARTITION_SPLIT => {
            write_sb(fi, fs, cw, subsize, bo);
            write_sb(fi, fs, cw, subsize, &BlockOffset{x: bo.x + hbs as usize, y: bo.y});
            write_sb(fi, fs, cw, subsize, &BlockOffset{x: bo.x, y: bo.y + hbs as usize});
            write_sb(fi, fs, cw, subsize, &BlockOffset{x: bo.x + hbs as usize, y: bo.y + hbs as usize});
        },
        _ => { assert!(false); },
    }

    cw.bc.update_partition_context(&bo, subsize, bsize);
}

fn encode_tile(fi: &FrameInvariants, fs: &mut FrameState) -> Vec<u8> {
    let w = ec::Writer::new();
    let fc = CDFContext::new();
    let bc = BlockContext::new(fi.sb_width*16, fi.sb_height*16);
    let mut cw = ContextWriter {
        w: w,
        fc: fc,
        bc: bc,
    };

    for sby in 0..fi.sb_height {
        for p in 0..3 {
            cw.bc.reset_left_coeff_context(p);
        }
        for sbx in 0..fi.sb_width {
            let sbo = SuperBlockOffset { x: sbx, y: sby };
            let bo = sbo.block_offset(0, 0);

            // partition with RDO-based mode decision
            search_partition(fi, fs, &mut cw, BlockSize::BLOCK_64X64, &bo);

            // Encode SuperBlock bitstream with decided modes, recursively
            write_sb(fi, fs, &mut cw, BlockSize::BLOCK_64X64, &bo);
        }
    }
    let mut h = cw.w.done();
    h.push(0); // superframe anti emulation
    h
}

fn encode_frame(sequence: &Sequence, fi: &FrameInvariants, fs: &mut FrameState, last_rec: &Option<Frame>) -> Vec<u8> {
    let mut packet = Vec::new();
    write_uncompressed_header(&mut packet, sequence, fi).unwrap();
    if fi.show_existing_frame {
        match last_rec {
            &Some(ref rec) => for p in 0..3 {
                fs.rec.planes[p].data.copy_from_slice(rec.planes[p].data.as_slice());
            },
            &None => (),
        }
    } else {
        let tile = encode_tile(fi, fs);
        packet.write(&tile).unwrap();
    }
    packet
}

/// Encode and write a frame.
pub fn process_frame(sequence: &Sequence, fi: &FrameInvariants,
                     output_file: &mut Write,
                     y4m_dec: &mut y4m::Decoder<Box<Read>>,
                     y4m_enc: Option<&mut y4m::Encoder<Box<Write>>>,
                     last_rec: &mut Option<Frame>) -> bool {
    let width = fi.width;
    let height = fi.height;
    match y4m_dec.read_frame() {
        Ok(y4m_frame) => {
            let y4m_y = y4m_frame.get_y_plane();
            let y4m_u = y4m_frame.get_u_plane();
            let y4m_v = y4m_frame.get_v_plane();
            eprintln!("{}", fi);
            let mut fs = FrameState::new(&fi);
            for y in 0..height {
                for x in 0..width {
                    let stride = fs.input.planes[0].cfg.stride;
                    fs.input.planes[0].data[y*stride+x] = y4m_y[y*width+x] as u16;
                }
            }
            for y in 0..height/2 {
                for x in 0..width/2 {
                    let stride = fs.input.planes[1].cfg.stride;
                    fs.input.planes[1].data[y*stride+x] = y4m_u[y*width/2+x] as u16;
                }
            }
            for y in 0..height/2 {
                for x in 0..width/2 {
                    let stride = fs.input.planes[2].cfg.stride;
                    fs.input.planes[2].data[y*stride+x] = y4m_v[y*width/2+x] as u16;
                }
            }
            let packet = encode_frame(&sequence, &fi, &mut fs, &last_rec);
            write_ivf_frame(output_file, fi.number, packet.as_ref());
            match y4m_enc {
                Some(mut y4m_enc) => {
                    let mut rec_y = vec![128 as u8; width*height];
                    let mut rec_u = vec![128 as u8; width*height/4];
                    let mut rec_v = vec![128 as u8; width*height/4];
                    for y in 0..height {
                        for x in 0..width {
                            let stride = fs.rec.planes[0].cfg.stride;
                            rec_y[y*width+x] = fs.rec.planes[0].data[y*stride+x] as u8;
                        }
                    }
                    for y in 0..height/2 {
                        for x in 0..width/2 {
                            let stride = fs.rec.planes[1].cfg.stride;
                            rec_u[y*width/2+x] = fs.rec.planes[1].data[y*stride+x] as u8;
                        }
                    }
                    for y in 0..height/2 {
                        for x in 0..width/2 {
                            let stride = fs.rec.planes[2].cfg.stride;
                            rec_v[y*width/2+x] = fs.rec.planes[2].data[y*stride+x] as u8;
                        }
                    }
                    let rec_frame = y4m::Frame::new([&rec_y, &rec_u, &rec_v], None);
                    y4m_enc.write_frame(&rec_frame).unwrap();
                }
                None => {}
            }
            *last_rec = Some(fs.rec);
            true
        },
        _ => false
    }
}
