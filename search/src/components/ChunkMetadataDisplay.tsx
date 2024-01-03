/* eslint-disable @typescript-eslint/no-unsafe-call */
import { For, Setter, Show, createMemo, createSignal } from "solid-js";
import {
  indirectHasOwnProperty,
  type ChunkBookmarksDTO,
  type ChunkCollectionDTO,
  type ChunkMetadataWithVotes,
  ChunkMetadata,
  ClientEnvsConfiguration,
} from "../../utils/apiTypes";
import { BiRegularChevronDown, BiRegularChevronUp } from "solid-icons/bi";
import sanitizeHtml from "sanitize-html";
import { VsFileSymlinkFile } from "solid-icons/vs";
import BookmarkPopover from "./BookmarkPopover";
import { FiEdit, FiTrash } from "solid-icons/fi";
import { formatDate, sanitzerOptions } from "./ScoreChunk";
import { Tooltip } from "./Atoms/Tooltip";
import CommunityBookmarkPopover from "./CommunityBookmarkPopover";
import {
  FaRegularFileCode,
  FaRegularFileImage,
  FaRegularFilePdf,
} from "solid-icons/fa";
import { FullScreenModal } from "./Atoms/FullScreenModal";
import { RiOthersCharacterRecognitionLine } from "solid-icons/ri";

export const getLocalTime = (strDate: string | Date) => {
  const utcDate = new Date(strDate);

  const timeZoneOffsetMinutes = new Date().getTimezoneOffset();

  const localTime = new Date(
    utcDate.getTime() - timeZoneOffsetMinutes * 60 * 1000,
  );

  return localTime;
};

export interface ChunkMetadataDisplayProps {
  totalCollectionPages: number;
  signedInUserId?: string;
  viewingUserId?: string;
  chunk: ChunkMetadataWithVotes | ChunkMetadata;
  chunkCollections: ChunkCollectionDTO[];
  bookmarks: ChunkBookmarksDTO[];
  setShowModal: Setter<boolean>;
  setShowConfirmModal: Setter<boolean>;
  fetchChunkCollections: () => void;
  setChunkCollections: Setter<ChunkCollectionDTO[]>;
  setOnDelete: Setter<() => void>;
  showExpand?: boolean;
}

const ChunkMetadataDisplay = (props: ChunkMetadataDisplayProps) => {
  const apiHost = import.meta.env.PUBLIC_API_HOST as string;
  const envs = JSON.parse(
    localStorage.getItem("clientConfig") ?? "{}",
  ) as ClientEnvsConfiguration;

  const frontMatterVals = (
    (envs.PUBLIC_FRONTMATTER_VALS as string | undefined) ??
    "link,tag_set,file_name,time_stamp"
  ).split(",");

  const linesBeforeShowMore =
    (envs.PUBLIC_LINES_BEFORE_SHOW_MORE as number | undefined) ?? 10;

  const [expanded, setExpanded] = createSignal(false);
  const [deleting, setDeleting] = createSignal(false);
  const [deleted, setDeleted] = createSignal(false);
  const [showImageModal, setShowImageModal] = createSignal(false);
  const [showMetadata, setShowMetadata] = createSignal(false);

  const onDelete = () => {
    const dataset = import.meta.env.PUBLIC_DATASET as string;
    if (props.signedInUserId !== props.viewingUserId) return;
    const curChunkId = props.chunk.id;

    props.setOnDelete(() => {
      return () => {
        setDeleting(true);
        void fetch(`${apiHost}/chunk/${curChunkId}`, {
          method: "DELETE",
          credentials: "include",
          headers: {
            "AF-Dataset": dataset,
          },
        }).then((response) => {
          setDeleting(false);
          if (response.ok) {
            setDeleted(true);
            return;
          }
          alert("Failed to delete chunk");
        });
      };
    });

    props.setShowConfirmModal(true);
  };

  const imgInformation = createMemo(() => {
    const imgRangeStartKey = envs.PUBLIC_IMAGE_RANGE_START_KEY;
    const imgRangeEndKey = envs.PUBLIC_IMAGE_RANGE_END_KEY;

    if (
      !imgRangeStartKey ||
      !props.chunk.metadata ||
      !indirectHasOwnProperty(props.chunk.metadata, imgRangeStartKey) ||
      !indirectHasOwnProperty(props.chunk.metadata, imgRangeEndKey)
    ) {
      return null;
    }

    // eslint-disable-next-line @typescript-eslint/no-unsafe-assignment, @typescript-eslint/no-unsafe-member-access, @typescript-eslint/no-explicit-any
    const imgRangeStartVal = (props.chunk.metadata as any)[
      imgRangeStartKey
    ] as unknown as string;
    // eslint-disable-next-line @typescript-eslint/no-unsafe-assignment, @typescript-eslint/no-unsafe-member-access, @typescript-eslint/no-explicit-any
    const imgRangeEndVal = (props.chunk.metadata as any)[
      imgRangeEndKey
    ] as unknown as string;
    const imgRangeStart = parseInt(imgRangeStartVal.replace(/\D+/g, ""), 10);
    const imgRangeEnd = parseInt(imgRangeEndVal.replace(/\D+/g, ""), 10);
    const imgRangePrefix = imgRangeStartVal.slice(
      0,
      -imgRangeStart.toString().length,
    );

    return {
      imgRangeStart,
      imgRangeEnd,
      imgRangePrefix,
    };
  });

  const useExpand = createMemo(() => {
    return props.chunk.content.split(" ").length > 20 * linesBeforeShowMore;
  });

  return (
    <>
      <Show when={!deleted()}>
        <div class="flex w-full flex-col items-center rounded-md bg-neutral-100 p-2 dark:bg-neutral-800">
          <div class="flex w-full flex-col space-y-2">
            <div class="flex h-fit items-center space-x-1">
              <div class="flex-1" />
              <Tooltip
                body={
                  <Show when={imgInformation()}>
                    <button
                      class="h-fit"
                      onClick={() => setShowImageModal(true)}
                      title="View Images"
                    >
                      <FaRegularFileImage class="h-5 w-5 fill-current" />
                    </button>
                  </Show>
                }
                tooltipText="View Full Document"
              />
              <Tooltip
                body={
                  <Show when={imgInformation()}>
                    <a
                      class="h-fit"
                      href={`${apiHost}/pdf_from_range/${
                        imgInformation()?.imgRangeStart ?? 0
                      }/${imgInformation()?.imgRangeEnd ?? 0}/${
                        imgInformation()?.imgRangePrefix ?? ""
                      }/${
                        // eslint-disable-next-line @typescript-eslint/no-unnecessary-condition
                        props.chunk.metadata?.file_name ??
                        imgInformation()?.imgRangeStart ??
                        "Arguflow PDF From Range"
                      }/false`}
                      target="_blank"
                      title="Open PDF"
                    >
                      <FaRegularFilePdf class="h-5 w-5 fill-current" />
                    </a>
                  </Show>
                }
                tooltipText="View PDF"
              />
              <Tooltip
                body={
                  <Show when={imgInformation()}>
                    <a
                      class="h-fit"
                      href={`${apiHost}/pdf_from_range/${
                        imgInformation()?.imgRangeStart ?? 0
                      }/${imgInformation()?.imgRangeEnd ?? 0}/${
                        imgInformation()?.imgRangePrefix ?? ""
                      }/${
                        // eslint-disable-next-line @typescript-eslint/no-unnecessary-condition
                        props.chunk.metadata?.file_name ??
                        imgInformation()?.imgRangeStart ??
                        "Arguflow PDF From Range"
                      }/true`}
                      target="_blank"
                      title="Open PDF"
                    >
                      <RiOthersCharacterRecognitionLine class="h-5 w-5 fill-current" />
                    </a>
                  </Show>
                }
                tooltipText="View PDF With OCR"
              />
              <Tooltip
                body={
                  <Show when={Object.keys(props.chunk.metadata ?? {}).length}>
                    <button
                      class="h-fit"
                      onClick={() => setShowMetadata(true)}
                      title="View Images"
                    >
                      <FaRegularFileCode class="h-5 w-5 fill-current" />
                    </button>
                  </Show>
                }
                tooltipText="View Full Metadata"
              />
              <Show when={props.signedInUserId == props.viewingUserId}>
                <button
                  classList={{
                    "h-fit text-red-700 dark:text-red-400": true,
                    "animate-pulse": deleting(),
                  }}
                  title="Delete"
                  onClick={() => onDelete()}
                >
                  <FiTrash class="h-5 w-5" />
                </button>
              </Show>
              <Show when={props.signedInUserId == props.viewingUserId}>
                <a title="Edit" href={`/chunk/edit/${props.chunk.id}`}>
                  <FiEdit class="h-5 w-5" />
                </a>
              </Show>
              <Tooltip
                body={
                  <a title="Open" href={`/chunk/${props.chunk.id}`}>
                    <VsFileSymlinkFile class="h-5 w-5 fill-current" />
                  </a>
                }
                tooltipText="Open in new tab"
              />
              <CommunityBookmarkPopover
                bookmarks={props.bookmarks.filter(
                  (bookmark) => bookmark.chunk_uuid == props.chunk.id,
                )}
              />

              <BookmarkPopover
                signedInUserId={props.signedInUserId}
                totalCollectionPages={props.totalCollectionPages}
                chunkCollections={props.chunkCollections}
                chunkMetadata={props.chunk}
                setLoginModal={props.setShowModal}
                bookmarks={props.bookmarks.filter(
                  (bookmark) => bookmark.chunk_uuid == props.chunk.id,
                )}
                setChunkCollections={props.setChunkCollections}
              />
            </div>
            <div class="flex w-full flex-col">
              <For each={frontMatterVals}>
                {(frontMatterVal) => (
                  <>
                    <Show when={props.chunk.link && frontMatterVal == "link"}>
                      <a
                        class="line-clamp-1 w-fit break-all text-magenta-500 underline dark:text-turquoise-400"
                        target="_blank"
                        href={props.chunk.link ?? ""}
                      >
                        {props.chunk.link}
                      </a>
                    </Show>
                    <Show
                      when={props.chunk.tag_set && frontMatterVal == "tag_set"}
                    >
                      <div class="flex space-x-2">
                        <span class="font-semibold text-neutral-800 dark:text-neutral-200">
                          Tag Set:{" "}
                        </span>
                        <span class="line-clamp-1 break-all">
                          {props.chunk.tag_set}
                        </span>
                      </div>
                    </Show>
                    <Show
                      when={
                        props.chunk.time_stamp && frontMatterVal == "time_stamp"
                      }
                    >
                      <div class="flex space-x-2">
                        <span class="font-semibold text-neutral-800 dark:text-neutral-200">
                          Time Stamp:{" "}
                        </span>
                        <span class="line-clamp-1 break-all">
                          {formatDate(new Date(props.chunk.time_stamp ?? ""))}
                        </span>
                      </div>
                    </Show>
                    <Show
                      when={
                        // eslint-disable-next-line @typescript-eslint/no-unsafe-assignment
                        frontMatterVal !== "link" &&
                        frontMatterVal !== "tag_set" &&
                        frontMatterVal !== "time_stamp" &&
                        props.chunk.metadata &&
                        indirectHasOwnProperty(
                          props.chunk.metadata,
                          frontMatterVal,
                        ) &&
                        // eslint-disable-next-line @typescript-eslint/no-unsafe-member-access, @typescript-eslint/no-explicit-any
                        (props.chunk.metadata as any)[frontMatterVal]
                      }
                    >
                      <div class="flex space-x-2">
                        <span class="font-semibold text-neutral-800 dark:text-neutral-200">
                          {frontMatterVal}:{" "}
                        </span>
                        <span class="line-clamp-1 break-all">
                          {props.chunk.metadata &&
                            indirectHasOwnProperty(
                              props.chunk.metadata,
                              frontMatterVal,
                            ) &&
                            // eslint-disable-next-line @typescript-eslint/no-unsafe-member-access, @typescript-eslint/no-explicit-any, @typescript-eslint/no-unsafe-call
                            (props.chunk.metadata as any)[
                              frontMatterVal
                            ].replace(/ +/g, " ")}
                        </span>
                      </div>
                    </Show>
                  </>
                )}
              </For>
            </div>
          </div>
          <div class="mb-1 h-1 w-full border-b border-neutral-300 dark:border-neutral-600" />
          <div
            classList={{
              "line-clamp-4 gradient-mask-b-0": useExpand() && !expanded(),
              "text-ellipsis max-w-[100%] break-words space-y-5 leading-normal !text-black dark:!text-white":
                true,
            }}
            style={
              useExpand() && !expanded()
                ? { "-webkit-line-clamp": linesBeforeShowMore }
                : {}
            }
            // eslint-disable-next-line solid/no-innerhtml
            innerHTML={sanitizeHtml(
              props.chunk.chunk_html !== undefined
                ? props.chunk.chunk_html
                    .replaceAll("line-height", "lh")
                    .replace("\n", " ")
                    .replace(`<br>`, " ")
                    .replace(`\\n`, " ")
                : "",
              // eslint-disable-next-line @typescript-eslint/no-unsafe-argument
              sanitzerOptions,
            )}
          />
          <Show when={useExpand()}>
            <button
              classList={{
                "ml-2 font-semibold": true,
                "animate-pulse": !props.showExpand,
              }}
              disabled={!props.showExpand}
              onClick={() => setExpanded((prev) => !prev)}
            >
              {expanded() ? (
                <div class="flex flex-row items-center">
                  <div>Show Less</div>{" "}
                  <BiRegularChevronUp class="h-8 w-8 fill-current" />
                </div>
              ) : (
                <div class="flex flex-row items-center">
                  <div>Show More</div>{" "}
                  <BiRegularChevronDown class="h-8 w-8 fill-current" />
                </div>
              )}
            </button>
          </Show>
        </div>
      </Show>
      <Show when={showImageModal()}>
        <FullScreenModal isOpen={showImageModal} setIsOpen={setShowImageModal}>
          <div class="flex max-h-[75vh] max-w-[75vw] flex-col space-y-2 overflow-auto">
            <For
              each={Array.from({
                length:
                  (imgInformation()?.imgRangeEnd ?? 0) -
                  (imgInformation()?.imgRangeStart ?? 0) +
                  1,
              })}
            >
              {(_, i) => (
                <img
                  class="mx-auto my-auto"
                  src={`${apiHost}/image/${
                    imgInformation()?.imgRangePrefix ?? ""
                  }${(imgInformation()?.imgRangeStart ?? 0) + i()}.png`}
                />
              )}
            </For>
          </div>
        </FullScreenModal>
      </Show>
      <Show when={showMetadata()}>
        <FullScreenModal isOpen={showMetadata} setIsOpen={setShowMetadata}>
          <div class="flex max-h-[60vh] max-w-[75vw] flex-col space-y-2 overflow-auto scrollbar-thin scrollbar-track-neutral-200 scrollbar-thumb-neutral-400 scrollbar-thumb-rounded-md dark:scrollbar-track-neutral-800 dark:scrollbar-thumb-neutral-600">
            <For each={Object.keys(props.chunk.metadata ?? {})}>
              {(metadataKey) => (
                <div class="flex flex-wrap space-x-2">
                  <span>{`"${metadataKey}":`}</span>
                  <span>{`"${
                    // eslint-disable-next-line @typescript-eslint/no-unsafe-member-access, @typescript-eslint/no-explicit-any, @typescript-eslint/restrict-template-expressions
                    typeof (props.chunk.metadata as any)[metadataKey] ===
                    "object"
                      ? JSON.stringify(
                          // eslint-disable-next-line @typescript-eslint/no-unsafe-member-access, @typescript-eslint/no-explicit-any
                          (props.chunk.metadata as any)[metadataKey],
                        )
                      : // eslint-disable-next-line @typescript-eslint/no-unsafe-member-access, @typescript-eslint/no-explicit-any
                        (props.chunk.metadata as any)[metadataKey]
                  }"`}</span>
                </div>
              )}
            </For>
          </div>
        </FullScreenModal>
      </Show>
    </>
  );
};

export default ChunkMetadataDisplay;