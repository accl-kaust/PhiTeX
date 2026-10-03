//! Saving a checkpoint's engine and loading it back in another process
//! (`DESIGN.md` §5.3, "On-disk checkpoints").
//!
//! [`Tex::save_state`] destructures the engine exhaustively, like the
//! state hash: a new field does not compile until it is saved (or listed
//! as rebuilt on load). The host and tracker are the new process's; the
//! caches (the `\csname` cache) start empty. A state recording memo
//! entries (`-memo`) is not saved: its tables are named from token lists.

use alloc::sync::Arc;
use partex_engine::persist::{Loader, Persist, Saver};

use crate::host::Host;
use crate::tex::Tex;
use crate::track::Tracker;

impl<H: Host, T: Tracker> Tex<H, T> {
    /// Save the engine's state; `false` (nothing written) if it cannot
    /// be (memo recording on).
    pub fn save_state(&self, s: &mut Saver) -> bool {
        let Tex {
            host: _,
            tracker: _,
            // (a value beside the stack and buffer, made again from them)
            input_values: _,
            params,
            xord,
            xchr,
            xprn,
            str_pool,
            str_start,
            pool_ptr,
            str_ptr,
            init_pool_ptr,
            init_str_ptr,
            str_index: _,
            skip: _,
            log_file,
            term_buf,
            write_file,
            selector,
            dig,
            tally,
            term_offset,
            file_offset,
            trick_buf,
            trick_count,
            first_count,
            arith_error,
            save_arith_error,
            remainder,
            line,
            buffer,
            first,
            last,
            max_buf_stack,
            interaction,
            deletions_allowed,
            set_box_allowed,
            history,
            error_count,
            help_line,
            help_ptr,
            use_err_help,
            special_printing,
            message_printing,
            no_convert,
            active_noconvert,
            cs_converting,
            font_in_short_display,
            depth_threshold,
            breadth_max,
            nest,
            max_nest_stack,
            cur_list,
            shown_mode,
            eqtb,
            eqtb_obj,
            glue_lineage,
            xeq_level,
            eqtb_top,
            old_setting,
            sys_time,
            sys_day,
            sys_month,
            sys_year,
            hash,
            hash_used,
            hash_top,
            hash_high,
            no_new_control_sequence,
            cs_count,
            cur_val,
            cur_glue,
            glue_origin,
            cur_toks,
            split_discards,
            par_loc,
            par_token,
            write_loc,
            mltex_enabled_p,
            enctex_enabled_p,
            interrupt,
            ok_to_interrupt,
            halting_on_error,
            edit_request,
            save_stack,
            save_ptr,
            save_eqtb,
            save_obj,
            max_save_stack,
            cur_level,
            cur_group,
            cur_boundary,
            mag_set,
            cur_cmd,
            cur_chr,
            cur_cs,
            cur_tok,
            input_stack,
            input_ptr,
            max_in_stack,
            cur_input,
            in_open,
            open_parens,
            synctex_tags,
            synctex_flags,
            synctex_root,
            synctex_shipped,
            input_file,
            line_stack,
            grp_stack,
            if_stack,
            eof_seen,
            pseudo_files,
            source_filename_stack,
            full_source_filename_stack,
            scanner_status,
            warning_index,
            def_ref,
            def_protected,
            param_stack,
            param_ptr,
            max_param_stack,
            align_state,
            base_ptr,
            expand_depth_count,
            cur_val_level,
            radix,
            cur_order,
            dead_cycles,
            out_rtl,
            lr_problems,
            last_badness,
            output_active,
            read_file,
            read_open,
            cur_name,
            cur_area,
            cur_ext,
            area_delimiter,
            ext_delimiter,
            quoted_filename,
            stop_at_space,
            name_of_file,
            format_ident,
            // (the format loaded: a value the session's engines share)
            format_data: _,
            force_eof,
            long_state,
            cond_stack,
            if_limit,
            cur_if,
            if_line,
            skip_line,
            cur_mark,
            job_name,
            log_opened,
            name_in_progress,
            output_file_name,
            log_name,
            fmem_ptr,
            font_ptr,
            dvi,
            write_open,
            pack_begin_line,
            adjust,
            hyph,
            page,
            align,
            etex_mode,
            epoch,
            is_in_csname,
            xregs,
            max_reg_num,
            max_reg_help_line,
            tok_pool: _,
            pstack_buf: _,
            empty_list: _,
            omit_list: _,
            arg_list,
            arg_active,
            token_only: _,
            preamble_list,
            preamble_active,
            prims,
            random,
            cur_box,
            after_token,
            memo,
            cs_cache: _,
            map_cache: _,
            hash_memo: _,
            line_log: _,
            log_lines: _,
            glyphs_used: _,
            record_objstms: _,
            objstms_written: _,
            glyphs_read: _,
            skip_tracked: _,
            class_hash: _,
            classes_read: _,
            classes_written: _,
            seals: _,
            seal_lines: _,
            canon_strings: _,
            cs_by_name: _,
            name_cells: _,
            name_log: _,
            font_cells: _,
            font_log: _,
            probe_names: _,
            seal_at: _,
            seal_log: _,
            stop_before_ship: _,
            ship_stop: _,
            stop_after_load: _,
            load_stop: _,
            defer_page: _,
            page_pending: _,
            graf_stop: _,
            step_began: _,
            par_start: _,
            fire_pending: _,
            defer_fire: _,
            window: _,
            window_start: _,
            window_cut: _,
            fresh_def,
            long_help_seen,
            cancel_boundary,
            checkpoint_every,
            commands,
            stop_at: _,
            stop_at_candidate: _,
            at_checkpoint,
            checkpoint_at,
            block_entered,
            dense: _,
            dense_at: _,
            dense_last: _,
            shipped,
            tounicode,
            pdf,
            fontmap,
            fonts_mapped,
            streams,
            fonts,
            diag,
            effects,
            // (glyph origins, `SyncTeX` and display lists: a session's side
            // channels, not saved; a state recording them is not saved)
            org,
            sync,
            dl,
        } = self;
        if memo.enabled
            || effects.as_ref().is_some_and(|e| !e.is_empty())
            || org.is_some()
            || sync.is_some()
            || dl.is_some()
        {
            return false;
        }
        s.mark("params");
        params.save(s);
        s.mark("xord");
        xord.save(s);
        s.mark("xchr");
        xchr.save(s);
        s.mark("xprn");
        xprn.save(s);
        s.mark("str_pool");
        str_pool.save(s);
        s.mark("str_start");
        str_start.save(s);
        s.mark("pool_ptr");
        pool_ptr.save(s);
        s.mark("str_ptr");
        str_ptr.save(s);
        s.mark("init_pool_ptr");
        init_pool_ptr.save(s);
        s.mark("init_str_ptr");
        init_str_ptr.save(s);
        s.mark("log_file");
        log_file.save(s);
        s.mark("term_buf");
        term_buf.save(s);
        s.mark("write_file");
        write_file.save(s);
        s.mark("selector");
        selector.save(s);
        s.mark("dig");
        dig.save(s);
        s.mark("tally");
        tally.save(s);
        s.mark("term_offset");
        term_offset.save(s);
        s.mark("file_offset");
        file_offset.save(s);
        s.mark("trick_buf");
        trick_buf.save(s);
        s.mark("trick_count");
        trick_count.save(s);
        s.mark("first_count");
        first_count.save(s);
        s.mark("arith_error");
        arith_error.save(s);
        s.mark("save_arith_error");
        save_arith_error.save(s);
        s.mark("remainder");
        remainder.save(s);
        s.mark("line");
        line.save(s);
        s.mark("buffer");
        buffer.save(s);
        s.mark("first");
        first.save(s);
        s.mark("last");
        last.save(s);
        s.mark("max_buf_stack");
        max_buf_stack.save(s);
        s.mark("interaction");
        interaction.save(s);
        s.mark("deletions_allowed");
        deletions_allowed.save(s);
        s.mark("set_box_allowed");
        set_box_allowed.save(s);
        s.mark("history");
        history.save(s);
        s.mark("error_count");
        error_count.save(s);
        s.mark("help_line");
        help_line.save(s);
        s.mark("help_ptr");
        help_ptr.save(s);
        s.mark("use_err_help");
        use_err_help.save(s);
        s.mark("special_printing");
        special_printing.save(s);
        s.mark("message_printing");
        message_printing.save(s);
        s.mark("no_convert");
        no_convert.save(s);
        s.mark("active_noconvert");
        active_noconvert.save(s);
        s.mark("cs_converting");
        cs_converting.save(s);
        s.mark("font_in_short_display");
        font_in_short_display.save(s);
        s.mark("depth_threshold");
        depth_threshold.save(s);
        s.mark("breadth_max");
        breadth_max.save(s);
        s.mark("nest");
        nest.save(s);
        s.mark("max_nest_stack");
        max_nest_stack.save(s);
        s.mark("cur_list");
        cur_list.save(s);
        s.mark("shown_mode");
        shown_mode.save(s);
        s.mark("eqtb");
        eqtb.save(s);
        s.mark("eqtb_obj");
        eqtb_obj.save(s);
        glue_lineage.save(s);
        s.mark("xeq_level");
        xeq_level.save(s);
        s.mark("eqtb_top");
        eqtb_top.save(s);
        s.mark("old_setting");
        old_setting.save(s);
        s.mark("sys_time");
        sys_time.save(s);
        s.mark("sys_day");
        sys_day.save(s);
        s.mark("sys_month");
        sys_month.save(s);
        s.mark("sys_year");
        sys_year.save(s);
        s.mark("hash");
        hash.save(s);
        s.mark("hash_used");
        hash_used.save(s);
        s.mark("hash_top");
        hash_top.save(s);
        s.mark("hash_high");
        hash_high.save(s);
        s.mark("no_new_control_sequence");
        no_new_control_sequence.save(s);
        s.mark("cs_count");
        cs_count.save(s);
        s.mark("cur_val");
        cur_val.save(s);
        s.mark("cur_glue");
        cur_glue.save(s);
        s.mark("glue_origin");
        glue_origin.save(s);
        cur_toks.save(s);
        s.mark("split_discards");
        split_discards.save(s);
        s.mark("par_loc");
        par_loc.save(s);
        s.mark("par_token");
        par_token.save(s);
        s.mark("write_loc");
        write_loc.save(s);
        s.mark("mltex_enabled_p");
        mltex_enabled_p.save(s);
        s.mark("enctex_enabled_p");
        enctex_enabled_p.save(s);
        s.mark("interrupt");
        interrupt.save(s);
        s.mark("ok_to_interrupt");
        ok_to_interrupt.save(s);
        s.mark("halting_on_error");
        halting_on_error.save(s);
        s.mark("edit_request");
        edit_request.save(s);
        s.mark("save_stack");
        save_stack.save(s);
        s.mark("save_ptr");
        save_ptr.save(s);
        s.mark("save_eqtb");
        save_eqtb.save(s);
        save_obj.save(s);
        s.mark("max_save_stack");
        max_save_stack.save(s);
        s.mark("cur_level");
        cur_level.save(s);
        s.mark("cur_group");
        cur_group.save(s);
        s.mark("cur_boundary");
        cur_boundary.save(s);
        s.mark("mag_set");
        mag_set.save(s);
        s.mark("cur_cmd");
        cur_cmd.save(s);
        s.mark("cur_chr");
        cur_chr.save(s);
        s.mark("cur_cs");
        cur_cs.save(s);
        s.mark("cur_tok");
        cur_tok.save(s);
        s.mark("input_stack");
        input_stack.save(s);
        s.mark("input_ptr");
        input_ptr.save(s);
        s.mark("max_in_stack");
        max_in_stack.save(s);
        s.mark("cur_input");
        cur_input.save(s);
        s.mark("in_open");
        in_open.save(s);
        s.mark("open_parens");
        open_parens.save(s);
        synctex_tags.save(s);
        synctex_flags.save(s);
        synctex_root.save(s);
        synctex_shipped.save(s);
        s.mark("input_file");
        input_file.save(s);
        s.mark("line_stack");
        line_stack.save(s);
        s.mark("grp_stack");
        grp_stack.save(s);
        s.mark("if_stack");
        if_stack.save(s);
        s.mark("eof_seen");
        eof_seen.save(s);
        s.mark("pseudo_files");
        pseudo_files.save(s);
        s.mark("source_filename_stack");
        source_filename_stack.save(s);
        s.mark("full_source_filename_stack");
        full_source_filename_stack.save(s);
        s.mark("scanner_status");
        scanner_status.save(s);
        s.mark("warning_index");
        warning_index.save(s);
        s.mark("def_ref");
        def_ref.save(s);
        def_protected.save(s);
        s.mark("param_stack");
        param_stack.save(s);
        s.mark("param_ptr");
        param_ptr.save(s);
        s.mark("max_param_stack");
        max_param_stack.save(s);
        s.mark("align_state");
        align_state.save(s);
        s.mark("base_ptr");
        base_ptr.save(s);
        s.mark("expand_depth_count");
        expand_depth_count.save(s);
        s.mark("cur_val_level");
        cur_val_level.save(s);
        s.mark("radix");
        radix.save(s);
        s.mark("cur_order");
        cur_order.save(s);
        s.mark("dead_cycles");
        dead_cycles.save(s);
        s.mark("out_rtl");
        out_rtl.save(s);
        s.mark("lr_problems");
        lr_problems.save(s);
        s.mark("last_badness");
        last_badness.save(s);
        s.mark("output_active");
        output_active.save(s);
        s.mark("read_file");
        read_file.save(s);
        s.mark("read_open");
        read_open.save(s);
        s.mark("cur_name");
        cur_name.save(s);
        s.mark("cur_area");
        cur_area.save(s);
        s.mark("cur_ext");
        cur_ext.save(s);
        s.mark("area_delimiter");
        area_delimiter.save(s);
        s.mark("ext_delimiter");
        ext_delimiter.save(s);
        s.mark("quoted_filename");
        quoted_filename.save(s);
        s.mark("stop_at_space");
        stop_at_space.save(s);
        s.mark("name_of_file");
        name_of_file.save(s);
        s.mark("format_ident");
        format_ident.save(s);
        s.mark("force_eof");
        force_eof.save(s);
        s.mark("long_state");
        long_state.save(s);
        s.mark("cond_stack");
        cond_stack.save(s);
        s.mark("if_limit");
        if_limit.save(s);
        s.mark("cur_if");
        cur_if.save(s);
        s.mark("if_line");
        if_line.save(s);
        s.mark("skip_line");
        skip_line.save(s);
        s.mark("cur_mark");
        cur_mark.save(s);
        s.mark("job_name");
        job_name.save(s);
        s.mark("log_opened");
        log_opened.save(s);
        s.mark("name_in_progress");
        name_in_progress.save(s);
        s.mark("output_file_name");
        output_file_name.save(s);
        s.mark("log_name");
        log_name.save(s);
        s.mark("fmem_ptr");
        fmem_ptr.save(s);
        s.mark("font_ptr");
        font_ptr.save(s);
        s.mark("dvi");
        dvi.save(s);
        s.mark("write_open");
        write_open.save(s);
        s.mark("pack_begin_line");
        pack_begin_line.save(s);
        s.mark("adjust");
        adjust.save(s);
        s.mark("hyph");
        hyph.save(s);
        s.mark("page");
        page.save(s);
        s.mark("align");
        align.save(s);
        s.mark("etex_mode");
        etex_mode.save(s);
        s.mark("epoch");
        epoch.save(s);
        s.mark("is_in_csname");
        is_in_csname.save(s);
        s.mark("xregs");
        xregs.save(s);
        s.mark("max_reg_num");
        max_reg_num.save(s);
        s.mark("max_reg_help_line");
        max_reg_help_line.save(s);
        s.mark("arg_list");
        arg_list.save(s);
        arg_active.save(s);
        s.mark("preamble_list");
        preamble_list.save(s);
        preamble_active.save(s);
        s.mark("prims");
        prims.save(s);
        s.mark("random");
        random.save(s);
        s.mark("cur_box");
        cur_box.save(s);
        s.mark("after_token");
        after_token.save(s);
        s.mark("fresh_def");
        fresh_def.save(s);
        s.mark("long_help_seen");
        long_help_seen.save(s);
        s.mark("cancel_boundary");
        cancel_boundary.save(s);
        s.mark("checkpoint_every");
        checkpoint_every.save(s);
        s.mark("commands");
        commands.save(s);
        s.mark("at_checkpoint");
        at_checkpoint.save(s);
        s.mark("checkpoint_at");
        checkpoint_at.save(s);
        s.mark("block_entered");
        block_entered.save(s);
        s.mark("shipped");
        shipped.save(s);
        s.mark("tounicode");
        tounicode.save(s);
        s.mark("pdf");
        pdf.save(s);
        s.mark("fontmap");
        fontmap.save(s);
        s.mark("fonts_mapped");
        fonts_mapped.save(s);
        s.mark("streams");
        streams.save(s);
        s.mark("fonts");
        fonts.save(s);
        s.mark("diag");
        diag.save(s);
        effects.is_some().save(s);
        true
    }

    /// An engine in the state `l` holds, with this host and tracker.
    pub fn load_state(l: &mut Loader, host: H, tracker: T) -> Option<Self> {
        Some(Tex {
            host,
            tracker,
            input_values: crate::input::InputValues::default(),
            params: Persist::load(l)?,
            xord: Persist::load(l)?,
            xchr: Persist::load(l)?,
            xprn: Persist::load(l)?,
            str_pool: Persist::load(l)?,
            str_start: Persist::load(l)?,
            pool_ptr: Persist::load(l)?,
            str_ptr: Persist::load(l)?,
            init_pool_ptr: Persist::load(l)?,
            init_str_ptr: Persist::load(l)?,
            // (an index of the pool, rebuilt as it is used)
            str_index: crate::strings::StrIndex::default(),
            skip: crate::skipcache::SkipCache::default(),
            log_file: Persist::load(l)?,
            term_buf: Persist::load(l)?,
            write_file: Persist::load(l)?,
            selector: Persist::load(l)?,
            dig: Persist::load(l)?,
            tally: Persist::load(l)?,
            term_offset: Persist::load(l)?,
            file_offset: Persist::load(l)?,
            trick_buf: Persist::load(l)?,
            trick_count: Persist::load(l)?,
            first_count: Persist::load(l)?,
            arith_error: Persist::load(l)?,
            save_arith_error: Persist::load(l)?,
            remainder: Persist::load(l)?,
            line: Persist::load(l)?,
            buffer: Persist::load(l)?,
            first: Persist::load(l)?,
            last: Persist::load(l)?,
            max_buf_stack: Persist::load(l)?,
            interaction: Persist::load(l)?,
            deletions_allowed: Persist::load(l)?,
            set_box_allowed: Persist::load(l)?,
            history: Persist::load(l)?,
            error_count: Persist::load(l)?,
            help_line: Persist::load(l)?,
            help_ptr: Persist::load(l)?,
            use_err_help: Persist::load(l)?,
            special_printing: Persist::load(l)?,
            message_printing: Persist::load(l)?,
            no_convert: Persist::load(l)?,
            active_noconvert: Persist::load(l)?,
            cs_converting: Persist::load(l)?,
            font_in_short_display: Persist::load(l)?,
            depth_threshold: Persist::load(l)?,
            breadth_max: Persist::load(l)?,
            nest: Persist::load(l)?,
            max_nest_stack: Persist::load(l)?,
            cur_list: Persist::load(l)?,
            shown_mode: Persist::load(l)?,
            eqtb: Persist::load(l)?,
            eqtb_obj: Persist::load(l)?,
            glue_lineage: Persist::load(l)?,
            xeq_level: Persist::load(l)?,
            eqtb_top: Persist::load(l)?,
            old_setting: Persist::load(l)?,
            sys_time: Persist::load(l)?,
            sys_day: Persist::load(l)?,
            sys_month: Persist::load(l)?,
            sys_year: Persist::load(l)?,
            hash: Persist::load(l)?,
            hash_used: Persist::load(l)?,
            hash_top: Persist::load(l)?,
            hash_high: Persist::load(l)?,
            no_new_control_sequence: Persist::load(l)?,
            cs_count: Persist::load(l)?,
            cur_val: Persist::load(l)?,
            cur_glue: Persist::load(l)?,
            glue_origin: Persist::load(l)?,
            cur_toks: Persist::load(l)?,
            split_discards: Persist::load(l)?,
            par_loc: Persist::load(l)?,
            par_token: Persist::load(l)?,
            write_loc: Persist::load(l)?,
            mltex_enabled_p: Persist::load(l)?,
            enctex_enabled_p: Persist::load(l)?,
            interrupt: Persist::load(l)?,
            ok_to_interrupt: Persist::load(l)?,
            halting_on_error: Persist::load(l)?,
            edit_request: Persist::load(l)?,
            save_stack: Persist::load(l)?,
            save_ptr: Persist::load(l)?,
            save_eqtb: Persist::load(l)?,
            save_obj: Persist::load(l)?,
            max_save_stack: Persist::load(l)?,
            cur_level: Persist::load(l)?,
            cur_group: Persist::load(l)?,
            cur_boundary: Persist::load(l)?,
            mag_set: Persist::load(l)?,
            cur_cmd: Persist::load(l)?,
            cur_chr: Persist::load(l)?,
            cur_cs: Persist::load(l)?,
            cur_tok: Persist::load(l)?,
            input_stack: Persist::load(l)?,
            input_ptr: Persist::load(l)?,
            max_in_stack: Persist::load(l)?,
            cur_input: Persist::load(l)?,
            in_open: Persist::load(l)?,
            open_parens: Persist::load(l)?,
            synctex_tags: Persist::load(l)?,
            synctex_flags: Persist::load(l)?,
            synctex_root: Persist::load(l)?,
            synctex_shipped: Persist::load(l)?,
            input_file: Persist::load(l)?,
            line_stack: Persist::load(l)?,
            grp_stack: Persist::load(l)?,
            if_stack: Persist::load(l)?,
            eof_seen: Persist::load(l)?,
            pseudo_files: Persist::load(l)?,
            source_filename_stack: Persist::load(l)?,
            full_source_filename_stack: Persist::load(l)?,
            scanner_status: Persist::load(l)?,
            warning_index: Persist::load(l)?,
            def_ref: Persist::load(l)?,
            def_protected: Persist::load(l)?,
            param_stack: Persist::load(l)?,
            param_ptr: Persist::load(l)?,
            max_param_stack: Persist::load(l)?,
            align_state: Persist::load(l)?,
            base_ptr: Persist::load(l)?,
            expand_depth_count: Persist::load(l)?,
            cur_val_level: Persist::load(l)?,
            radix: Persist::load(l)?,
            cur_order: Persist::load(l)?,
            dead_cycles: Persist::load(l)?,
            out_rtl: Persist::load(l)?,
            lr_problems: Persist::load(l)?,
            last_badness: Persist::load(l)?,
            output_active: Persist::load(l)?,
            read_file: Persist::load(l)?,
            read_open: Persist::load(l)?,
            cur_name: Persist::load(l)?,
            cur_area: Persist::load(l)?,
            cur_ext: Persist::load(l)?,
            area_delimiter: Persist::load(l)?,
            ext_delimiter: Persist::load(l)?,
            quoted_filename: Persist::load(l)?,
            stop_at_space: Persist::load(l)?,
            name_of_file: Persist::load(l)?,
            format_ident: Persist::load(l)?,
            format_data: None,
            force_eof: Persist::load(l)?,
            long_state: Persist::load(l)?,
            cond_stack: Persist::load(l)?,
            if_limit: Persist::load(l)?,
            cur_if: Persist::load(l)?,
            if_line: Persist::load(l)?,
            skip_line: Persist::load(l)?,
            cur_mark: Persist::load(l)?,
            job_name: Persist::load(l)?,
            log_opened: Persist::load(l)?,
            name_in_progress: Persist::load(l)?,
            output_file_name: Persist::load(l)?,
            log_name: Persist::load(l)?,
            fmem_ptr: Persist::load(l)?,
            font_ptr: Persist::load(l)?,
            dvi: Persist::load(l)?,
            write_open: Persist::load(l)?,
            pack_begin_line: Persist::load(l)?,
            adjust: Persist::load(l)?,
            hyph: Persist::load(l)?,
            page: Persist::load(l)?,
            align: Persist::load(l)?,
            etex_mode: Persist::load(l)?,
            epoch: Persist::load(l)?,
            is_in_csname: Persist::load(l)?,
            xregs: Persist::load(l)?,
            max_reg_num: Persist::load(l)?,
            max_reg_help_line: Persist::load(l)?,
            tok_pool: alloc::vec::Vec::new(),
            pstack_buf: alloc::vec::Vec::new(),
            empty_list: partex_engine::node::TokenList::shared(&[]),
            omit_list: partex_engine::node::TokenList::shared(&[crate::web::END_TEMPLATE_TOKEN]),
            arg_list: Persist::load(l)?,
            arg_active: Persist::load(l)?,
            token_only: false,
            preamble_list: Persist::load(l)?,
            preamble_active: Persist::load(l)?,
            prims: Persist::load(l)?,
            random: Persist::load(l)?,
            cur_box: Persist::load(l)?,
            after_token: Persist::load(l)?,
            memo: crate::memo::Memo::new(false),
            cs_cache: crate::hash::CsCache::default(),
            map_cache: crate::fontmap::MapCache::default(),
            hash_memo: crate::hashmemo::HashMemo::default(),
            line_log: alloc::vec::Vec::new(),
            log_lines: false,
            glyphs_used: alloc::vec::Vec::new(),
            record_objstms: false,
            objstms_written: alloc::vec::Vec::new(),
            glyphs_read: false,
            skip_tracked: false,
            class_hash: 0,
            classes_read: false,
            classes_written: false,
            seals: crate::seal::SealTable::default(),
            seal_lines: false,
            canon_strings: false,
            cs_by_name: false,
            name_cells: false,
            name_log: alloc::vec::Vec::new(),
            font_cells: false,
            font_log: alloc::vec::Vec::new(),
            probe_names: false,
            seal_at: (0, 0),
            seal_log: alloc::vec::Vec::new(),
            stop_before_ship: false,
            ship_stop: 0,
            stop_after_load: false,
            load_stop: 0,
            defer_page: false,
            page_pending: false,
            graf_stop: false,
            step_began: 0,
            par_start: false,
            fire_pending: false,
            defer_fire: false,
            window: 0,
            window_start: 0,
            window_cut: None,
            fresh_def: Persist::load(l)?,
            long_help_seen: Persist::load(l)?,
            cancel_boundary: Persist::load(l)?,
            checkpoint_every: Persist::load(l)?,
            commands: Persist::load(l)?,
            stop_at: 0,
            stop_at_candidate: false,
            at_checkpoint: Persist::load(l)?,
            checkpoint_at: Persist::load(l)?,
            block_entered: Persist::load(l)?,
            dense: false,
            dense_at: (0, 0),
            dense_last: false,
            shipped: Persist::load(l)?,
            tounicode: Persist::load(l)?,
            pdf: Persist::load(l)?,
            fontmap: Persist::load(l)?,
            fonts_mapped: Persist::load(l)?,
            streams: Persist::load(l)?,
            fonts: Persist::load(l)?,
            diag: Persist::load(l)?,
            effects: bool::load(l)?.then(alloc::vec::Vec::new),
            org: None,
            sync: None,
            dl: None,
        })
    }

    /// What [`Tex::save_state`] leaves out that a machine's snapshots
    /// hold (machine mode's own state: sealed lines, skip classes, the
    /// switches it runs with); `false` (nothing written) if the engine is
    /// in the middle of something a snapshot never holds (object streams
    /// recorded for a session's splice).
    pub fn save_machine_extras(&self, s: &mut Saver) -> bool {
        if !self.objstms_written.is_empty() {
            return false;
        }
        self.line_log.save(s);
        self.log_lines.save(s);
        self.glyphs_used.save(s);
        self.record_objstms.save(s);
        self.glyphs_read.save(s);
        self.skip_tracked.save(s);
        self.class_hash.save(s);
        self.classes_read.save(s);
        self.classes_written.save(s);
        self.seals.save(s);
        self.seal_lines.save(s);
        self.canon_strings.save(s);
        self.cs_by_name.save(s);
        self.name_cells.save(s);
        self.name_log.save(s);
        self.font_cells.save(s);
        self.font_log.save(s);
        self.probe_names.save(s);
        self.seal_at.save(s);
        self.seal_log.save(s);
        self.stop_before_ship.save(s);
        self.ship_stop.save(s);
        self.par_start.save(s);
        self.fire_pending.save(s);
        self.defer_fire.save(s);
        self.stop_at.save(s);
        self.stop_at_candidate.save(s);
        self.dense.save(s);
        self.dense_at.save(s);
        self.dense_last.save(s);
        self.effects.save(s);
        true
    }

    /// Load what [`Tex::save_machine_extras`] saved.
    pub fn load_machine_extras(&mut self, l: &mut Loader) -> Option<()> {
        self.line_log = Persist::load(l)?;
        self.log_lines = Persist::load(l)?;
        self.glyphs_used = Persist::load(l)?;
        self.record_objstms = Persist::load(l)?;
        self.glyphs_read = Persist::load(l)?;
        self.skip_tracked = Persist::load(l)?;
        self.class_hash = Persist::load(l)?;
        self.classes_read = Persist::load(l)?;
        self.classes_written = Persist::load(l)?;
        self.seals = Persist::load(l)?;
        self.seal_lines = Persist::load(l)?;
        self.canon_strings = Persist::load(l)?;
        self.cs_by_name = Persist::load(l)?;
        self.name_cells = Persist::load(l)?;
        self.name_log = Persist::load(l)?;
        self.font_cells = Persist::load(l)?;
        self.font_log = Persist::load(l)?;
        self.probe_names = Persist::load(l)?;
        self.seal_at = Persist::load(l)?;
        self.seal_log = Persist::load(l)?;
        self.stop_before_ship = Persist::load(l)?;
        self.ship_stop = Persist::load(l)?;
        self.par_start = Persist::load(l)?;
        self.fire_pending = Persist::load(l)?;
        self.defer_fire = Persist::load(l)?;
        self.stop_at = Persist::load(l)?;
        self.stop_at_candidate = Persist::load(l)?;
        self.dense = Persist::load(l)?;
        self.dense_at = Persist::load(l)?;
        self.dense_last = Persist::load(l)?;
        self.effects = Persist::load(l)?;
        Some(())
    }

    /// This engine saved and loaded back, with its host and tracker
    /// (`None` if it cannot be saved): a check that a saved state is the
    /// state.
    /// `seed`: values outside the engine it may share (the contents of
    /// files read), kept identical rather than copied.
    pub fn round_trip(&self, seed: &[Arc<[u8]>]) -> Option<Self>
    where
        H: Clone,
        T: Clone,
    {
        let mut s = Saver::new();
        let seed: alloc::vec::Vec<_> = seed.iter().filter(|a| s.preshare(a)).collect();
        if !self.save_state(&mut s) {
            return None;
        }
        let bytes = s.into_bytes();
        let mut l = Loader::new(&bytes);
        for a in seed {
            l.preshare(Arc::clone(a));
        }
        let t = Self::load_state(&mut l, self.host.clone(), self.tracker.clone())?;
        l.at_end().then_some(t)
    }
}
