//! Parts 48 and 49 of `mf.web` with `mf.ch`: dumping and undumping the
//! tables (the base file's layout is this crate's own, its contents
//! `mf.web`'s), the main program, initialization.

use crate::arith::round_unscaled;
use crate::print::BANNER;
use crate::{AlphaFile, Config, FourQuarters, InStateRecord, Io, Mf, R, Stop, Word, consts::*};

const UNDEFINED_LABEL: i32 = LIG_TABLE_SIZE;
const BASE_MAGIC: i32 = 0x5732_4D46;
const ENGINE_NAME: &[u8] = b"metafont";

impl Mf {
    /// A METAFONT ready to run: `mf.ch`'s setup of the bound variables
    /// and arrays (§1204).
    pub fn new(cfg: Config, io: Box<dyn Io>) -> Mf {
        let error_line = cfg.error_line.min(SSUP_ERROR_LINE);
        let screen_width = cfg.screen_width.min(SSUP_SCREEN_WIDTH);
        let screen_depth = cfg.screen_depth.min(SSUP_SCREEN_DEPTH);
        let main_memory = cfg.main_memory.clamp(INF_MAIN_MEMORY, SUP_MAIN_MEMORY);
        let buf_size = cfg.buf_size.clamp(INF_BUF_SIZE, SUP_BUF_SIZE);
        let mem_top = MEM_MIN + main_memory - 1;
        let ini = cfg.ini;
        let mut dump_name = vec![b' '];
        dump_name.extend_from_slice(cfg.dump_name.as_bytes());
        dump_name.extend_from_slice(b".base");
        Mf {
            mem_top,
            mem_max: mem_top,
            buf_size,
            error_line,
            half_error_line: cfg.half_error_line,
            max_print_line: cfg.max_print_line,
            screen_width,
            screen_depth,
            gf_buf_size: cfg.gf_buf_size,
            ini_version: ini,
            bad: 0,
            xord: [0; 256],
            xchr: [0; 256],
            xprn: [0; 256],
            name_of_file: Vec::new(),
            name_length: 0,
            buffer: vec![0; buf_size as usize + 2],
            first: 0,
            last: 0,
            max_buf_stack: 0,
            str_pool: vec![0; POOL_SIZE as usize + 1],
            str_start: vec![0; MAX_STRINGS as usize + 1],
            pool_ptr: 0,
            str_ptr: 0,
            init_pool_ptr: 0,
            init_str_ptr: 0,
            max_pool_ptr: 0,
            max_str_ptr: 0,
            str_ref: vec![0; MAX_STRINGS as usize + 1],
            log_file: Vec::new(),
            log_file_name: Vec::new(),
            selector: TERM_ONLY,
            dig: [0; 23],
            tally: 0,
            term_offset: 0,
            file_offset: 0,
            trick_buf: vec![0; error_line as usize + 1],
            trick_count: 0,
            first_count: 0,
            interaction: ERROR_STOP_MODE,
            deletions_allowed: true,
            history: FATAL_ERROR_STOP,
            error_count: 0,
            help_line: [""; 6],
            help_ptr: 0,
            use_err_help: false,
            err_help: 0,
            interrupt: 0,
            ok_to_interrupt: true,
            halting_on_error_p: false,
            arith_error: false,
            two_to_the: [0; 31],
            spec_log: [0; 29],
            spec_atan: [0; 27],
            n_sin: 0,
            n_cos: 0,
            randoms: [0; 55],
            j_random: 0,
            mem: if ini {
                vec![Word::default(); (mem_top - MEM_MIN + 1) as usize]
            } else {
                Vec::new()
            },
            lo_mem_max: 0,
            hi_mem_min: 0,
            var_used: 0,
            dyn_used: 0,
            avail: 0,
            mem_end: 0,
            rover: 0,
            internal: vec![0; MAX_INTERNAL as usize + 1],
            int_name: vec![0; MAX_INTERNAL as usize + 1],
            int_ptr: 0,
            old_setting: 0,
            sys_time: 0,
            sys_day: 0,
            sys_month: 0,
            sys_year: 0,
            char_class: [0; 256],
            hash_used: 0,
            st_count: 0,
            hash: vec![Word::default(); HASH_END as usize + 1],
            eqtb: vec![Word::default(); HASH_END as usize + 1],
            g_pointer: 0,
            big_node_size: [0; 2],
            save_ptr: 0,
            path_tail: 0,
            delta_x: vec![0; PATH_SIZE as usize + 1],
            delta_y: vec![0; PATH_SIZE as usize + 1],
            delta: vec![0; PATH_SIZE as usize + 1],
            psi: vec![0; PATH_SIZE as usize + 2],
            theta: vec![0; PATH_SIZE as usize + 1],
            uu: vec![0; PATH_SIZE as usize + 1],
            st: 0,
            ct: 0,
            sf: 0,
            cf: 0,
            vv: vec![0; PATH_SIZE as usize + 1],
            ww: vec![0; PATH_SIZE as usize + 1],
            move_: vec![0; MOVE_SIZE as usize + 1],
            move_ptr: 0,
            bisect_stack: vec![0; BISTACK_SIZE as usize + 1],
            bisect_ptr: 0,
            cur_edges: 0,
            cur_wt: 0,
            trace_x: 0,
            trace_y: 0,
            trace_yy: 0,
            octant: 0,
            cur_x: 0,
            cur_y: 0,
            cur_spec: 0,
            turning_number: 0,
            cur_pen: 0,
            cur_path_type: 0,
            max_allowed: 0,
            before: vec![0; MAX_WIGGLE as usize + 1],
            after: vec![0; MAX_WIGGLE as usize + 1],
            node_to_round: vec![0; MAX_WIGGLE as usize + 1],
            cur_rounding_ptr: 0,
            max_rounding_ptr: 0,
            cur_gran: 0,
            octant_number: [0; 9],
            octant_code: [0; 9],
            rev_turns: false,
            y_corr: [0; 9],
            xy_corr: [0; 9],
            z_corr: [0; 9],
            x_corr: [0; 9],
            m0: 0,
            n0: 0,
            m1: 0,
            n1: 0,
            d0: 0,
            d1: 0,
            env_move: vec![0; MOVE_SIZE as usize + 1],
            tol_step: 0,
            cur_t: 0,
            cur_tt: 0,
            time_to_go: 0,
            max_t: 0,
            delx: 0,
            dely: 0,
            tol: 0,
            uv: 0,
            xy: 0,
            three_l: 0,
            appr_t: 0,
            appr_tt: 0,
            screen_started: false,
            screen_ok: false,
            window_open: [false; 16],
            left_col: [0; 16],
            right_col: [0; 16],
            top_row: [0; 16],
            bot_row: [0; 16],
            m_window: [0; 16],
            n_window: [0; 16],
            window_time: [0; 16],
            row_transition: vec![0; screen_width as usize + 2],
            serial_no: 0,
            fix_needed: false,
            watch_coefs: true,
            dep_final: 0,
            cur_cmd: 0,
            cur_mod: 0,
            cur_sym: 0,
            input_stack: vec![InStateRecord::default(); STACK_SIZE as usize + 1],
            input_ptr: 0,
            max_in_stack: 0,
            cur_input: InStateRecord::default(),
            in_open: 0,
            open_parens: 0,
            input_file: vec![AlphaFile::default(); MAX_IN_OPEN as usize + 1],
            line: 0,
            line_stack: vec![0; MAX_IN_OPEN as usize + 1],
            full_source_filename_stack: vec![0; MAX_IN_OPEN as usize + 1],
            param_stack: vec![0; PARAM_SIZE as usize + 1],
            param_ptr: 0,
            max_param_stack: 0,
            file_ptr: 0,
            scanner_status: 0,
            warning_info: 0,
            force_eof: false,
            bg_loc: 0,
            eg_loc: 0,
            cond_ptr: 0,
            if_limit: 0,
            cur_if: 0,
            if_line: 0,
            loop_ptr: 0,
            cur_name: 0,
            cur_area: 0,
            cur_ext: 0,
            area_delimiter: 0,
            ext_delimiter: 0,
            quoted_filename: false,
            stop_at_space: true,
            mf_base_default: dump_name,
            job_name: 0,
            log_opened: false,
            log_name: 0,
            gf_ext: 0,
            cur_type: 0,
            cur_exp: 0,
            max_c: [0; 2],
            max_ptr: [0; 2],
            max_link: [0; 2],
            var_flag: 0,
            txx: 0,
            txy: 0,
            tyx: 0,
            tyy: 0,
            tx: 0,
            ty: 0,
            start_sym: 0,
            long_help_seen: false,
            tfm_file: Vec::new(),
            metric_file_name: 0,
            bc: 0,
            ec: 0,
            tfm_width: [0; 256],
            tfm_height: [0; 256],
            tfm_depth: [0; 256],
            tfm_ital_corr: [0; 256],
            char_exists: [false; 256],
            char_tag: [0; 256],
            char_remainder: [0; 256],
            header_byte: vec![0; HEADER_SIZE as usize + 1],
            lig_kern: vec![FourQuarters::default(); LIG_TABLE_SIZE as usize + 1],
            nl: 0,
            kern: vec![0; MAX_KERNS as usize + 1],
            nk: 0,
            exten: [FourQuarters::default(); 256],
            ne: 0,
            param: vec![0; MAX_FONT_DIMEN as usize + 1],
            np: 0,
            nw: 0,
            nh: 0,
            nd: 0,
            ni: 0,
            perturbation: 0,
            excess: 0,
            dimen_head: [0; 5],
            max_tfm_dimen: 0,
            tfm_changed: 0,
            skip_table: [0; 256],
            lk_started: false,
            bchar: 0,
            bch_label: 0,
            ll: 0,
            lll: 0,
            label_loc: [0; 257],
            label_char: [0; 257],
            label_ptr: 0,
            gf_file: Vec::new(),
            gf_file_name: Vec::new(),
            gf_min_m: 0,
            gf_max_m: 0,
            gf_min_n: 0,
            gf_max_n: 0,
            gf_prev_ptr: 0,
            total_chars: 0,
            char_ptr: [0; 256],
            gf_dx: [0; 256],
            gf_dy: [0; 256],
            gf_buf: vec![0; cfg.gf_buf_size.max(8) as usize + 1],
            half_buf: 0,
            gf_limit: 0,
            gf_ptr: 0,
            gf_offset: 0,
            boc_c: 0,
            boc_p: 0,
            output_file_name: 0,
            base_ident: 0,
            base_file: Vec::new(),
            base_pos: 0,
            ready_already: 0,
            edit_name_start: 0,
            edit_name_length: 0,
            edit_line: 0,
            term_out: Vec::new(),
            outputs: Vec::new(),
            term_in: AlphaFile {
                data: cfg.terminal_input.clone(),
                pos: 0,
            },
            io,
            cfg,
        }
    }

    /// Runs METAFONT (§1204): the history (`spotless` to
    /// `fatal_error_stop`) as web2c's exit status reflects it.
    pub fn run(&mut self) -> i32 {
        let _ = self.main_body();
        self.history
    }

    fn main_body(&mut self) -> R {
        self.history = FATAL_ERROR_STOP;
        // §14 and the other consistency checks
        self.bad = 0;
        if self.half_error_line < 30 || self.half_error_line > self.error_line - 15 {
            self.bad = 1;
        }
        if self.max_print_line < 60 {
            self.bad = 2;
        }
        if self.gf_buf_size % 8 != 0 {
            self.bad = 3;
        }
        if MEM_MIN + 1100 > self.mem_top {
            self.bad = 4;
        }
        if self.bad > 0 {
            let s = format!(
                "Ouch---my internal constants have been clobbered!---case {}",
                self.bad
            );
            self.wterm_ln(s.as_bytes());
            return Err(Stop);
        }
        self.initialize();
        if self.ini_version {
            if !self.get_strings_started()? {
                return Err(Stop);
            }
            self.init_tab();
            self.init_prim()?;
            self.init_str_ptr = self.str_ptr;
            self.init_pool_ptr = self.pool_ptr;
            self.max_str_ptr = self.str_ptr;
            self.max_pool_ptr = self.pool_ptr;
            self.fix_date_and_time();
        }
        self.ready_already = 314_159;
        // §55, §61, §783, §792: initialize the output routines
        self.selector = TERM_ONLY;
        self.tally = 0;
        self.term_offset = 0;
        self.file_offset = 0;
        self.wterm(BANNER.as_bytes());
        let v = self.cfg.version_string.clone();
        self.wterm(v.as_bytes());
        if self.base_ident == 0 {
            let s = format!(" (preloaded base={})", self.cfg.dump_name);
            self.wterm_ln(s.as_bytes());
        } else {
            self.slow_print(self.base_ident);
            self.print_ln();
        }
        if let Some(t) = self.cfg.translate.as_ref().map(|t| t.0.clone()) {
            self.wterm(b"(");
            self.wterm(t.as_bytes());
            self.wterm_ln(b")");
        }
        self.job_name = 0;
        self.log_opened = false;
        self.output_file_name = 0;
        // §1211
        {
            // §657
            self.input_ptr = 0;
            self.max_in_stack = 0;
            self.in_open = 0;
            self.open_parens = 0;
            self.max_buf_stack = 0;
            self.param_ptr = 0;
            self.max_param_stack = 0;
            self.first = 1;
            self.set_start(1);
            self.set_index(0);
            self.line = 0;
            self.set_name(0);
            self.force_eof = false;
            if !self.init_terminal()? {
                return Err(Stop);
            }
            self.set_limit(self.last);
            self.first = self.last + 1;
            // §660
            self.scanner_status = NORMAL;
            if self.base_ident == 0 || self.buffer[self.loc() as usize] == b'&' {
                if self.base_ident != 0 {
                    self.initialize();
                }
                if !self.open_base_file()? {
                    return Err(Stop);
                }
                if !self.load_base_file()? {
                    return Err(Stop);
                }
                while self.loc() < self.limit() && self.buffer[self.loc() as usize] == b' ' {
                    self.set_loc(self.loc() + 1);
                }
            }
            let lim = self.limit();
            self.buffer[lim as usize] = b'%';
            self.fix_date_and_time();
            self.init_randoms(self.sys_time + self.sys_day * UNITY);
            // §70
            if self.interaction == BATCH_MODE {
                self.selector = NO_PRINT;
            } else {
                self.selector = TERM_ONLY;
            }
            if self.loc() < self.limit() && self.buffer[self.loc() as usize] != b'\\' {
                self.start_input()?;
            }
        }
        self.history = SPOTLESS;
        if self.start_sym > 0 {
            self.cur_sym = self.start_sym;
            self.back_input()?;
        }
        self.main_control()?;
        self.final_cleanup()?;
        self.close_files_and_terminate()?;
        Ok(())
    }

    /// `initialize` (§8): every `Set initial values` section.
    fn initialize(&mut self) {
        // §21-§23 (mf.ch): xchr the identity, xord its inverse
        for i in 0..256usize {
            self.xchr[i] = i as u8;
        }
        for i in 0..256usize {
            self.xord[i] = 0o177;
        }
        for i in 0o200..=0o377usize {
            self.xord[self.xchr[i] as usize] = i as u8;
        }
        for i in 0..=0o176usize {
            self.xord[self.xchr[i] as usize] = i as u8;
        }
        for i in 0..256usize {
            self.xprn[i] = u8::from(self.cfg.eight_bit || (i >= 32 && i <= 126));
        }
        if let Some((_, xord, xchr, xprn)) = self.cfg.translate.clone() {
            self.xord = *xord;
            self.xchr = *xchr;
            self.xprn = *xprn;
        }
        // §69
        self.interaction = match self.cfg.interaction_option {
            None => ERROR_STOP_MODE,
            Some(i) => i32::from(i),
        };
        // §72, §75, §92, §98
        self.deletions_allowed = true;
        self.error_count = 0;
        self.help_ptr = 0;
        self.use_err_help = false;
        self.err_help = 0;
        self.interrupt = 0;
        self.ok_to_interrupt = true;
        self.arith_error = false;
        // §131, §138
        self.init_arith_tables();
        // §191
        for k in 1..=MAX_GIVEN_INTERNAL as usize {
            self.internal[k] = 0;
        }
        self.int_ptr = MAX_GIVEN_INTERNAL;
        // §199
        for k in 0..256usize {
            self.char_class[k] = INVALID_CLASS;
        }
        for k in b'0'..=b'9' {
            self.char_class[k as usize] = DIGIT_CLASS;
        }
        self.char_class[b'.' as usize] = PERIOD_CLASS;
        self.char_class[b' ' as usize] = SPACE_CLASS;
        self.char_class[b'%' as usize] = PERCENT_CLASS;
        self.char_class[b'"' as usize] = STRING_CLASS;
        self.char_class[b',' as usize] = 5;
        self.char_class[b';' as usize] = 6;
        self.char_class[b'(' as usize] = 7;
        self.char_class[b')' as usize] = RIGHT_PAREN_CLASS;
        for k in b'A'..=b'Z' {
            self.char_class[k as usize] = LETTER_CLASS;
        }
        for k in b'a'..=b'z' {
            self.char_class[k as usize] = LETTER_CLASS;
        }
        self.char_class[b'_' as usize] = LETTER_CLASS;
        for c in b"<=>:|" {
            self.char_class[*c as usize] = 10;
        }
        self.char_class[b'`' as usize] = 11;
        self.char_class[b'\'' as usize] = 11;
        self.char_class[b'+' as usize] = 12;
        self.char_class[b'-' as usize] = 12;
        self.char_class[b'/' as usize] = 13;
        self.char_class[b'*' as usize] = 13;
        self.char_class[b'\\' as usize] = 13;
        self.char_class[b'!' as usize] = 14;
        self.char_class[b'?' as usize] = 14;
        for c in b"#&@$" {
            self.char_class[*c as usize] = 15;
        }
        self.char_class[b'^' as usize] = 16;
        self.char_class[b'~' as usize] = 16;
        self.char_class[b'[' as usize] = LEFT_BRACKET_CLASS;
        self.char_class[b']' as usize] = RIGHT_BRACKET_CLASS;
        self.char_class[b'{' as usize] = 19;
        self.char_class[b'}' as usize] = 19;
        self.char_class[TAB as usize] = SPACE_CLASS;
        self.char_class[FORM_FEED as usize] = SPACE_CLASS;
        // §202
        let w1 = Word { lh: 0, rh: 0 };
        let e1 = Word {
            lh: TAG_TOKEN,
            rh: NULL,
        };
        for k in 1..=HASH_END as usize {
            self.hash[k] = w1;
            self.eqtb[k] = e1;
        }
        // §231
        self.big_node_size = [TRANSFORM_NODE_SIZE, PAIR_NODE_SIZE];
        // §251
        self.save_ptr = NULL;
        // §428
        self.max_rounding_ptr = 0;
        // §449
        self.octant_code = [
            0,
            FIRST_OCTANT,
            SECOND_OCTANT,
            THIRD_OCTANT,
            FOURTH_OCTANT,
            FIFTH_OCTANT,
            SIXTH_OCTANT,
            SEVENTH_OCTANT,
            EIGHTH_OCTANT,
        ];
        for k in 1..=8 {
            self.octant_number[self.octant_code[k] as usize] = k as i32;
        }
        // §456
        self.rev_turns = false;
        // §462
        let set = |a: &mut [i32; 9], o: i32, v: i32| a[o as usize] = v;
        for (o, x, y, xy) in [
            (FIRST_OCTANT, 0, 0, 0),
            (SECOND_OCTANT, 0, 0, 1),
            (THIRD_OCTANT, -1, 1, 0),
            (FOURTH_OCTANT, 1, 0, 1),
            (FIFTH_OCTANT, 0, 1, 1),
            (SIXTH_OCTANT, 0, 1, 0),
            (SEVENTH_OCTANT, 1, 0, 1),
            (EIGHTH_OCTANT, -1, 1, 0),
        ] {
            set(&mut self.x_corr, o, x);
            set(&mut self.y_corr, o, y);
            set(&mut self.xy_corr, o, xy);
        }
        for k in 1..=8 {
            self.z_corr[k] = self.xy_corr[k] - self.x_corr[k];
        }
        // §570, §573
        self.screen_started = false;
        self.screen_ok = false;
        for k in 0..16 {
            self.window_open[k] = false;
            self.window_time[k] = 0;
        }
        // §593
        self.fix_needed = false;
        self.watch_coefs = true;
        // §739, §753
        self.cond_ptr = NULL;
        self.if_limit = NORMAL;
        self.cur_if = 0;
        self.if_line = 0;
        self.loop_ptr = NULL;
        // §797, §822, §1078, §1085
        self.cur_exp = 0;
        self.var_flag = 0;
        self.start_sym = 0;
        self.long_help_seen = false;
        // §1097
        for k in 0..256 {
            self.tfm_width[k] = 0;
            self.tfm_height[k] = 0;
            self.tfm_depth[k] = 0;
            self.tfm_ital_corr[k] = 0;
            self.char_exists[k] = false;
            self.char_tag[k] = NO_TAG;
            self.char_remainder[k] = 0;
            self.skip_table[k] = UNDEFINED_LABEL;
        }
        for k in 1..=HEADER_SIZE as usize {
            self.header_byte[k] = -1;
        }
        self.bc = 255;
        self.ec = 0;
        self.nl = 0;
        self.nk = 0;
        self.ne = 0;
        self.np = 0;
        self.internal[BOUNDARY_CHAR as usize] = -UNITY;
        self.bch_label = UNDEFINED_LABEL;
        self.label_loc[0] = -1;
        self.label_ptr = 0;
        // §1150, §1153
        self.gf_prev_ptr = 0;
        self.total_chars = 0;
        self.half_buf = self.gf_buf_size / 2;
        self.gf_limit = self.gf_buf_size;
        self.gf_ptr = 0;
        self.gf_offset = 0;
        // §1184, §1215
        self.base_ident = 0;
        self.edit_name_start = 0;
        self.stop_at_space = true;
        self.halting_on_error_p = false;
    }

    /// `init_tab` (§1210): every `Initialize table entries` section.
    fn init_tab(&mut self) {
        // §176
        self.rover = LO_MEM_STAT_MAX + 1;
        self.set_link(self.rover, EMPTY_FLAG);
        self.set_node_size(self.rover, 1000);
        self.set_llink(self.rover, self.rover);
        self.set_rlink(self.rover, self.rover);
        self.lo_mem_max = self.rover + 1000;
        self.set_link(self.lo_mem_max, NULL);
        self.set_info(self.lo_mem_max, NULL);
        for k in self.hi_mem_stat_min()..=self.mem_top {
            self.mem[k as usize] = self.mem[self.lo_mem_max as usize];
        }
        self.avail = NULL;
        self.mem_end = self.mem_top;
        self.hi_mem_min = self.hi_mem_stat_min();
        self.var_used = LO_MEM_STAT_MAX + 1 - MEM_MIN;
        self.dyn_used = self.mem_top + 1 - self.hi_mem_min;
        // §193
        for (k, s) in [
            (TRACING_TITLES, "tracingtitles"),
            (TRACING_EQUATIONS, "tracingequations"),
            (TRACING_CAPSULES, "tracingcapsules"),
            (TRACING_CHOICES, "tracingchoices"),
            (TRACING_SPECS, "tracingspecs"),
            (TRACING_PENS, "tracingpens"),
            (TRACING_COMMANDS, "tracingcommands"),
            (TRACING_RESTORES, "tracingrestores"),
            (TRACING_MACROS, "tracingmacros"),
            (TRACING_EDGES, "tracingedges"),
            (TRACING_OUTPUT, "tracingoutput"),
            (TRACING_STATS, "tracingstats"),
            (TRACING_ONLINE, "tracingonline"),
            (YEAR, "year"),
            (MONTH, "month"),
            (DAY, "day"),
            (TIME, "time"),
            (CHAR_CODE, "charcode"),
            (CHAR_EXT, "charext"),
            (CHAR_WD, "charwd"),
            (CHAR_HT, "charht"),
            (CHAR_DP, "chardp"),
            (CHAR_IC, "charic"),
            (CHAR_DX, "chardx"),
            (CHAR_DY, "chardy"),
            (DESIGN_SIZE, "designsize"),
            (HPPP, "hppp"),
            (VPPP, "vppp"),
            (X_OFFSET, "xoffset"),
            (Y_OFFSET, "yoffset"),
            (PAUSING, "pausing"),
            (SHOWSTOPPING, "showstopping"),
            (FONTMAKING, "fontmaking"),
            (PROOFING, "proofing"),
            (SMOOTHING, "smoothing"),
            (AUTOROUNDING, "autorounding"),
            (GRANULARITY, "granularity"),
            (FILLIN, "fillin"),
            (TURNING_CHECK, "turningcheck"),
            (WARNING_CHECK, "warningcheck"),
            (BOUNDARY_CHAR, "boundarychar"),
        ] {
            self.int_name[k as usize] = self.pstr(s);
        }
        // §203
        self.hash_used = FROZEN_INACCESSIBLE;
        self.st_count = 0;
        for (k, s) in [
            (FROZEN_BAD_VARDEF, "a bad variable"),
            (FROZEN_FI, "fi"),
            (FROZEN_END_GROUP, "endgroup"),
            (FROZEN_END_DEF, "enddef"),
            (FROZEN_END_FOR, "endfor"),
            (FROZEN_SEMICOLON, ";"),
            (FROZEN_COLON, ":"),
            (FROZEN_SLASH, "/"),
            (FROZEN_LEFT_BRACKET, "["),
            (FROZEN_RIGHT_DELIMITER, ")"),
            (FROZEN_INACCESSIBLE, " INACCESSIBLE"),
        ] {
            let n = self.pstr(s);
            self.set_text(k, n);
        }
        self.set_eq_type(FROZEN_RIGHT_DELIMITER, RIGHT_DELIMITER);
        // §229
        self.set_attr_loc(END_ATTR, HASH_END + 1);
        self.set_parent(END_ATTR, NULL);
        // §324
        let s = self.sentinel();
        self.set_info(s, MAX_HALFWORD);
        // §475
        self.set_ref_count(NULL_PEN, NULL);
        self.set_link(NULL_PEN, NULL);
        self.set_info(NULL_PEN + 1, 1);
        self.set_link(NULL_PEN + 1, NULL_COORDS);
        for k in NULL_PEN + 2..=NULL_PEN + 8 {
            self.mem[k as usize] = self.mem[(NULL_PEN + 1) as usize];
        }
        self.set_max_offset(NULL_PEN, 0);
        self.set_link(NULL_COORDS, NULL_COORDS);
        self.set_knil(NULL_COORDS, NULL_COORDS);
        self.set_x_coord(NULL_COORDS, 0);
        self.set_y_coord(NULL_COORDS, 0);
        // §587
        self.serial_no = 0;
        self.set_link(DEP_HEAD, DEP_HEAD);
        self.set_prev_dep(DEP_HEAD, DEP_HEAD);
        self.set_info(DEP_HEAD, NULL);
        self.set_dep_list(DEP_HEAD, NULL);
        // §702
        self.set_name_type(BAD_VARDEF, ROOT);
        self.set_link(BAD_VARDEF, FROZEN_BAD_VARDEF);
        self.set_equiv(FROZEN_BAD_VARDEF, BAD_VARDEF);
        self.set_eq_type(FROZEN_BAD_VARDEF, TAG_TOKEN);
        // §759
        self.set_eq_type(FROZEN_REPEAT_LOOP, REPEAT_LOOP + OUTER_TAG);
        let s = self.pstr(" ENDFOR");
        self.set_text(FROZEN_REPEAT_LOOP, s);
        // §911
        self.set_name_type(TEMP_VAL, CAPSULE);
        // §1116, §1127
        self.set_value(INF_VAL, FRACTION_FOUR);
        self.set_value(ZERO_VAL, 0);
        self.set_info(ZERO_VAL, 0);
        // §1185
        if self.ini_version {
            self.base_ident = self.pstr(" (INIMF)");
        }
    }

    // §1209
    fn final_cleanup(&mut self) -> R {
        let c = self.cur_mod;
        if self.job_name == 0 {
            self.open_log_file()?;
        }
        while self.input_ptr > 0 {
            if self.token_state() {
                self.end_token_list()?;
            } else {
                self.end_file_reading()?;
            }
        }
        while self.loop_ptr != NULL {
            self.stop_iteration()?;
        }
        while self.open_parens > 0 {
            self.prints(" )");
            self.open_parens -= 1;
        }
        while self.cond_ptr != NULL {
            self.print_nl("(end occurred when ");
            self.print_cmd_mod(FI_OR_ELSE, self.cur_if)?;
            if self.if_line != 0 {
                self.prints(" on line ");
                self.print_int(self.if_line);
            }
            self.prints(" was incomplete)");
            self.if_line = self.mem[(self.cond_ptr + 1) as usize].rh;
            self.cur_if = self.name_type(self.cond_ptr);
            self.loop_ptr = self.cond_ptr;
            self.cond_ptr = self.link(self.cond_ptr);
            self.free_node(self.loop_ptr, IF_NODE_SIZE);
        }
        if self.history != SPOTLESS
            && (self.history == WARNING_ISSUED || self.interaction < ERROR_STOP_MODE)
            && self.selector == TERM_AND_LOG
        {
            self.selector = TERM_ONLY;
            self.print_nl("(see the transcript file for additional information)");
            self.selector = TERM_AND_LOG;
        }
        if c == 1 {
            if self.ini_version {
                return self.store_base_file();
            }
            self.print_nl("(dump is performed only by INIMF)");
        }
        Ok(())
    }

    pub(crate) fn wlog_ln(&mut self, s: &[u8]) {
        self.wlog(s);
        self.wlog_cr();
    }

    // §1205
    pub(crate) fn close_files_and_terminate(&mut self) -> R {
        if self.int(TRACING_STATS) > 0 && self.log_opened {
            // §1208
            self.wlog_ln(b" ");
            self.wlog_ln(b"Here is how much of METAFONT's memory you used:");
            let n = self.max_str_ptr - self.init_str_ptr;
            let s = format!(
                " {} string{} out of {}",
                n,
                if self.max_str_ptr != self.init_str_ptr + 1 {
                    "s"
                } else {
                    ""
                },
                MAX_STRINGS - self.init_str_ptr
            );
            self.wlog_ln(s.as_bytes());
            let s = format!(
                " {} string characters out of {}",
                self.max_pool_ptr - self.init_pool_ptr,
                POOL_SIZE - self.init_pool_ptr
            );
            self.wlog_ln(s.as_bytes());
            let s = format!(
                " {} words of memory out of {}",
                self.lo_mem_max - MEM_MIN + self.mem_end - self.hi_mem_min + 2,
                self.mem_end + 1 - MEM_MIN
            );
            self.wlog_ln(s.as_bytes());
            let s = format!(" {} symbolic tokens out of {}", self.st_count, HASH_SIZE);
            self.wlog_ln(s.as_bytes());
            let s = format!(
                " {}i,{}n,{}r,{}p,{}b stack positions out of {}i,{}n,{}r,{}p,{}b",
                self.max_in_stack,
                self.int_ptr,
                self.max_rounding_ptr,
                self.max_param_stack,
                self.max_buf_stack + 1,
                STACK_SIZE,
                MAX_INTERNAL,
                MAX_WIGGLE,
                PARAM_SIZE,
                self.buf_size
            );
            self.wlog_ln(s.as_bytes());
        }
        // §1206
        if self.gf_prev_ptr > 0 || self.int(FONTMAKING) > 0 {
            // §1207
            self.rover = LO_MEM_STAT_MAX + 1;
            self.set_link(self.rover, EMPTY_FLAG);
            self.lo_mem_max = self.hi_mem_min - 1;
            if self.lo_mem_max - self.rover > MAX_HALFWORD {
                self.lo_mem_max = MAX_HALFWORD + self.rover;
            }
            self.set_node_size(self.rover, self.lo_mem_max - self.rover);
            self.set_llink(self.rover, self.rover);
            self.set_rlink(self.rover, self.rover);
            self.set_link(self.lo_mem_max, NULL);
            self.set_info(self.lo_mem_max, NULL);
            // §1124
            let th = self.temp_head();
            self.set_link(th, INF_VAL);
            for k in self.bc..=self.ec {
                let k = k as usize;
                if self.char_exists[k] {
                    self.tfm_width[k] = self.sort_in(self.tfm_width[k])?;
                }
            }
            self.nw = self.skimp(255) + 1;
            self.dimen_head[1] = self.link(th);
            if self.perturbation >= 0o10000 {
                self.tfm_warning(CHAR_WD);
            }
            self.fix_design_size();
            self.fix_check_sum();
            if self.int(FONTMAKING) > 0 {
                self.massage_hdi()?;
                self.internal[FONTMAKING as usize] = 0;
                self.finish_tfm()?;
            }
            if self.gf_prev_ptr > 0 {
                self.finish_gf()?;
            }
        }
        if self.log_opened {
            self.wlog_cr();
            self.log_opened = false;
            let log = std::mem::take(&mut self.log_file);
            let name = self.log_file_name.clone();
            self.outputs.push((name, log));
            self.selector -= 2;
            if self.selector == TERM_ONLY {
                self.print_nl("Transcript written on ");
                self.print_file_name(0, self.log_name, 0);
                self.print_char(b'.');
            }
        }
        self.print_ln();
        Ok(())
    }

    // §1186 (with §1199)
    fn store_base_file(&mut self) -> R {
        // §1199
        self.selector = NEW_STRING;
        self.prints(" (preloaded base=");
        self.print(self.job_name);
        self.print_char(b' ');
        self.print_int(round_unscaled(self.int(YEAR)));
        self.print_char(b'.');
        self.print_int(round_unscaled(self.int(MONTH)));
        self.print_char(b'.');
        self.print_int(round_unscaled(self.int(DAY)));
        self.print_char(b')');
        if self.interaction == BATCH_MODE {
            self.selector = LOG_ONLY;
        } else {
            self.selector = TERM_AND_LOG;
        }
        self.str_room(1)?;
        self.base_ident = self.make_string()?;
        self.str_ref[self.base_ident as usize] = MAX_STR_REF as u8;
        self.pack_job_name(self.pstr(".base"));
        let base_name = self.name_of_file.clone();
        self.print_nl("Beginning to dump on file ");
        let s = self.make_name_string()?;
        self.slow_print(s);
        self.flush_string(self.str_ptr - 1);
        self.print_nl("");
        self.slow_print(self.base_ident);
        let mut d: Vec<u8> = Vec::new();
        let di = |d: &mut Vec<u8>, x: i32| d.extend_from_slice(&x.to_le_bytes());
        // §1190
        di(&mut d, BASE_MAGIC);
        di(&mut d, ENGINE_NAME.len() as i32);
        d.extend_from_slice(ENGINE_NAME);
        d.extend_from_slice(&self.xord);
        d.extend_from_slice(&self.xchr);
        d.extend_from_slice(&self.xprn);
        di(&mut d, MEM_MIN);
        di(&mut d, self.mem_top);
        di(&mut d, HASH_SIZE);
        di(&mut d, HASH_PRIME);
        di(&mut d, MAX_IN_OPEN);
        // §1192
        di(&mut d, self.pool_ptr);
        di(&mut d, self.str_ptr);
        for k in 0..=self.str_ptr as usize {
            di(&mut d, self.str_start[k]);
        }
        d.extend_from_slice(&self.str_pool[..self.pool_ptr as usize]);
        self.print_ln();
        self.print_int(self.str_ptr);
        self.prints(" strings of total length ");
        self.print_int(self.pool_ptr);
        // §1194
        self.sort_avail()?;
        self.var_used = 0;
        di(&mut d, self.lo_mem_max);
        di(&mut d, self.rover);
        let dw = |d: &mut Vec<u8>, w: Word| {
            d.extend_from_slice(&w.lh.to_le_bytes());
            d.extend_from_slice(&w.rh.to_le_bytes());
        };
        let mut p = MEM_MIN;
        let mut q = self.rover;
        let mut x = 0;
        loop {
            for k in p..=q + 1 {
                dw(&mut d, self.mem[k as usize]);
            }
            x = x + q + 2 - p;
            self.var_used = self.var_used + q - p;
            p = q + self.node_size(q);
            q = self.rlink(q);
            if q == self.rover {
                break;
            }
        }
        self.var_used = self.var_used + self.lo_mem_max - p;
        self.dyn_used = self.mem_end + 1 - self.hi_mem_min;
        for k in p..=self.lo_mem_max {
            dw(&mut d, self.mem[k as usize]);
        }
        x = x + self.lo_mem_max + 1 - p;
        di(&mut d, self.hi_mem_min);
        di(&mut d, self.avail);
        for k in self.hi_mem_min..=self.mem_end {
            dw(&mut d, self.mem[k as usize]);
        }
        x = x + self.mem_end + 1 - self.hi_mem_min;
        let mut p = self.avail;
        while p != NULL {
            self.dyn_used -= 1;
            p = self.link(p);
        }
        di(&mut d, self.var_used);
        di(&mut d, self.dyn_used);
        self.print_ln();
        self.print_int(x);
        self.prints(" memory locations dumped; current usage is ");
        self.print_int(self.var_used);
        self.print_char(b'&');
        self.print_int(self.dyn_used);
        // §1196
        di(&mut d, self.hash_used);
        self.st_count = FROZEN_INACCESSIBLE - 1 - self.hash_used;
        for p in 1..=self.hash_used {
            if self.text(p) != 0 {
                di(&mut d, p);
                dw(&mut d, self.hash[p as usize]);
                dw(&mut d, self.eqtb[p as usize]);
                self.st_count += 1;
            }
        }
        for p in self.hash_used + 1..=HASH_END {
            dw(&mut d, self.hash[p as usize]);
            dw(&mut d, self.eqtb[p as usize]);
        }
        di(&mut d, self.st_count);
        self.print_ln();
        self.print_int(self.st_count);
        self.prints(" symbolic tokens");
        // §1198
        di(&mut d, self.int_ptr);
        for k in 1..=self.int_ptr as usize {
            di(&mut d, self.internal[k]);
            di(&mut d, self.int_name[k]);
        }
        di(&mut d, self.start_sym);
        di(&mut d, self.interaction);
        di(&mut d, self.base_ident);
        di(&mut d, self.bg_loc);
        di(&mut d, self.eg_loc);
        di(&mut d, self.serial_no);
        di(&mut d, 69069);
        self.internal[TRACING_STATS as usize] = 0;
        // §1201
        self.outputs.push((base_name, d));
        Ok(())
    }

    // §1187
    fn load_base_file(&mut self) -> R<bool> {
        let d = std::mem::take(&mut self.base_file);
        let ok = self.undump(&d);
        if ok.is_none() {
            self.wterm_ln(b"(Fatal base file error; I'm stymied)");
            return Ok(false);
        }
        Ok(true)
    }

    fn undump(&mut self, d: &[u8]) -> Option<()> {
        let mut pos = 0usize;
        let ui = |pos: &mut usize| -> Option<i32> {
            let b = d.get(*pos..*pos + 4)?;
            *pos += 4;
            Some(i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        };
        macro_rules! undump {
            ($lo:expr, $hi:expr) => {{
                let x = ui(&mut pos)?;
                if x < $lo || x > $hi {
                    return None;
                }
                x
            }};
        }
        macro_rules! wd {
            () => {{
                let lh = ui(&mut pos)?;
                let rh = ui(&mut pos)?;
                Word { lh, rh }
            }};
        }
        macro_rules! bytes {
            ($n:expr) => {{
                let n = $n;
                let b = d.get(pos..pos + n)?;
                pos += n;
                b
            }};
        }
        if ui(&mut pos)? != BASE_MAGIC {
            return None;
        }
        let n = undump!(0, 256) as usize;
        if bytes!(n) != ENGINE_NAME {
            return None;
        }
        let xord = bytes!(256).to_vec();
        let xchr = bytes!(256).to_vec();
        let xprn = bytes!(256).to_vec();
        if self.cfg.translate.is_none() {
            self.xord.copy_from_slice(&xord);
            self.xchr.copy_from_slice(&xchr);
            self.xprn.copy_from_slice(&xprn);
            if self.cfg.eight_bit {
                self.xprn = [1; 256];
            }
        }
        if ui(&mut pos)? != MEM_MIN {
            return None;
        }
        self.mem_top = ui(&mut pos)?;
        if self.mem_max < self.mem_top {
            self.mem_max = self.mem_top;
        }
        if MEM_MIN + 1100 > self.mem_top {
            return None;
        }
        self.mem = vec![Word::default(); (self.mem_max - MEM_MIN + 1) as usize];
        if ui(&mut pos)? != HASH_SIZE || ui(&mut pos)? != HASH_PRIME || ui(&mut pos)? != MAX_IN_OPEN
        {
            return None;
        }
        // §1193
        self.pool_ptr = undump!(0, POOL_SIZE);
        self.str_ptr = undump!(0, MAX_STRINGS);
        for k in 0..=self.str_ptr as usize {
            self.str_start[k] = undump!(0, self.pool_ptr);
            self.str_ref[k] = MAX_STR_REF as u8;
        }
        let pool = bytes!(self.pool_ptr as usize);
        self.str_pool[..self.pool_ptr as usize].copy_from_slice(pool);
        self.init_str_ptr = self.str_ptr;
        self.init_pool_ptr = self.pool_ptr;
        self.max_str_ptr = self.str_ptr;
        self.max_pool_ptr = self.pool_ptr;
        // §1195
        self.lo_mem_max = undump!(LO_MEM_STAT_MAX + 1000, self.hi_mem_stat_min() - 1);
        self.rover = undump!(LO_MEM_STAT_MAX + 1, self.lo_mem_max);
        let mut p = MEM_MIN;
        let mut q = self.rover;
        loop {
            for k in p..=q + 1 {
                self.mem[k as usize] = wd!();
            }
            if self.node_size(q) > self.lo_mem_max - q
                || self.rlink(q) > self.lo_mem_max
                || (q >= self.rlink(q) && self.rlink(q) != self.rover)
            {
                return None;
            }
            p = q + self.node_size(q);
            q = self.rlink(q);
            if q == self.rover {
                break;
            }
        }
        for k in p..=self.lo_mem_max {
            self.mem[k as usize] = wd!();
        }
        self.hi_mem_min = undump!(self.lo_mem_max + 1, self.hi_mem_stat_min());
        self.avail = undump!(NULL, self.mem_top);
        self.mem_end = self.mem_top;
        for k in self.hi_mem_min..=self.mem_end {
            self.mem[k as usize] = wd!();
        }
        self.var_used = ui(&mut pos)?;
        self.dyn_used = ui(&mut pos)?;
        // §1197
        self.hash_used = undump!(1, FROZEN_INACCESSIBLE);
        let mut p = 0;
        loop {
            p = undump!(p + 1, self.hash_used);
            self.hash[p as usize] = wd!();
            self.eqtb[p as usize] = wd!();
            if p == self.hash_used {
                break;
            }
        }
        for p in self.hash_used + 1..=HASH_END {
            self.hash[p as usize] = wd!();
            self.eqtb[p as usize] = wd!();
        }
        self.st_count = ui(&mut pos)?;
        // §1199
        self.int_ptr = undump!(MAX_GIVEN_INTERNAL, MAX_INTERNAL);
        for k in 1..=self.int_ptr as usize {
            self.internal[k] = ui(&mut pos)?;
            self.int_name[k] = undump!(0, self.str_ptr);
        }
        self.start_sym = undump!(0, FROZEN_INACCESSIBLE);
        self.interaction = undump!(BATCH_MODE, ERROR_STOP_MODE);
        if let Some(i) = self.cfg.interaction_option {
            self.interaction = i32::from(i);
        }
        self.base_ident = undump!(0, self.str_ptr);
        self.bg_loc = undump!(1, HASH_END);
        self.eg_loc = undump!(1, HASH_END);
        self.serial_no = ui(&mut pos)?;
        if ui(&mut pos)? != 69069 {
            return None;
        }
        Some(())
    }
}
