use crate::{
  chunk_graph::ChunkGraph, stages::link_stage::LinkStageOutput,
  types::module_render_output::ModuleRenderOutput,
  utils::render_normal_module::render_normal_module, SharedOptions,
};

use anyhow::Result;
use rolldown_common::{Chunk, RenderedChunk};
use rolldown_sourcemap::{ConcatSource, RawSource, SourceMap, SourceMapSource};
use rolldown_utils::rayon::{IntoParallelRefIterator, ParallelIterator};
use rustc_hash::FxHashMap;
use sugar_path::SugarPath;

pub struct ChunkRenderReturn {
  pub code: String,
  pub map: Option<SourceMap>,
  pub rendered_chunk: RenderedChunk,
}

use super::{
  generate_rendered_chunk, render_chunk_exports::render_chunk_exports,
  render_chunk_imports::render_chunk_imports,
};

#[allow(clippy::unnecessary_wraps, clippy::cast_possible_truncation)]
pub async fn render_chunk(
  this: &Chunk,
  options: &SharedOptions,
  graph: &LinkStageOutput,
  chunk_graph: &ChunkGraph,
) -> Result<ChunkRenderReturn> {
  let mut rendered_modules = FxHashMap::default();
  let mut concat_source = ConcatSource::default();

  concat_source.add_source(Box::new(RawSource::new(render_chunk_imports(
    this,
    graph,
    chunk_graph,
  ))));

  this
    .modules
    .par_iter()
    .copied()
    .map(|id| &graph.module_table.normal_modules[id])
    .filter_map(|m| {
      render_normal_module(m, &graph.ast_table[m.id], m.resource_id.expect_file().as_ref(), options)
    })
    .collect::<Vec<_>>()
    .into_iter()
    .for_each(|module_render_output| {
      let ModuleRenderOutput {
        module_path,
        module_pretty_path,
        rendered_module,
        rendered_content,
        sourcemap,
        lines_count,
      } = module_render_output;
      concat_source.add_source(Box::new(RawSource::new(format!("// {module_pretty_path}",))));
      if let Some(sourcemap) = sourcemap {
        concat_source.add_source(Box::new(SourceMapSource::new(
          rendered_content,
          sourcemap,
          lines_count,
        )));
      } else {
        concat_source.add_source(Box::new(RawSource::new(rendered_content)));
      }
      rendered_modules.insert(module_path.to_string(), rendered_module);
    });
  let rendered_chunk = generate_rendered_chunk(this, graph, options, rendered_modules);

  // add banner
  if let Some(banner) = options.banner.as_ref() {
    if let Some(banner_txt) = banner.call(&rendered_chunk).await? {
      concat_source.add_prepend_source(Box::new(RawSource::new(banner_txt)));
    }
  }

  if let Some(exports) = render_chunk_exports(this, graph, options) {
    concat_source.add_source(Box::new(RawSource::new(exports)));
  }

  // add footer
  if let Some(footer) = options.footer.as_ref() {
    if let Some(footer_txt) = footer.call(&rendered_chunk).await? {
      concat_source.add_source(Box::new(RawSource::new(footer_txt)));
    }
  }

  let (content, mut map) = concat_source.content_and_sourcemap();

  if let Some(map) = map.as_mut() {
    // Here file path is generated by chunk file name template, it maybe including path segments.
    // So here need to read it's parent directory as file_dir.
    let file_path = options.cwd.as_path().join(&options.dir).join(
      this
        .preliminary_filename
        .as_deref()
        .expect("chunk file name should be generated before rendering"),
    );
    let file_dir = file_path.parent().expect("chunk file name should have a parent");

    let paths =
      map.get_sources().map(|source| source.as_path().relative(file_dir)).collect::<Vec<_>>();
    // Here not normalize the windows path, the rollup `sourcemap_path_transform` options need to original path.
    let sources = paths.iter().map(|x| x.to_string_lossy()).collect::<Vec<_>>();
    map.set_sources(sources.iter().map(std::convert::AsRef::as_ref).collect::<Vec<_>>());
  }

  Ok(ChunkRenderReturn { code: content, map, rendered_chunk })
}